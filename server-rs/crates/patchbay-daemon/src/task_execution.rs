//! Machine-level task claim and execution orchestration.
//!
//! This is the control-plane half of Go `daemon.go`'s `pollLoop`,
//! `runBatchPoller`, `handleTask`, cancellation watcher, and terminal result
//! delivery. Provider-specific preparation/execution remains behind the
//! required [`DaemonTaskExecutionHost`] boundary: there is intentionally no
//! default runner and no pretend execution path.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::activity::DaemonActivity;
use crate::client::{is_task_not_found_anyhow, is_transient_error, Client, TaskCancelAck};
use crate::execenv::execenv::{predict_root_dir, write_gc_meta, GCMetaKind, GcMeta};
use crate::local_directory::local_directory_assignment_for_task;
use crate::manager::DaemonControl;
use crate::reconcile::ReconcileBroadcaster;
use crate::repocache::{CancelCause, Ctx};
use crate::types::{RuntimeExecutionTarget, Task, TaskResult};
use crate::wakeup::TaskWakeup;

const DEFAULT_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(5);
const TASK_SLOT_CAPACITY_BACKOFF: Duration = Duration::from_millis(250);
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Details that survive only the server-cancelled path. Provider execution
/// sets this when finalization had to preserve an uncommitted worktree; the
/// cancel acknowledgement is then the only durable pointer to that work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CancelledRunDeliveryFailure {
    pub error_message: String,
    pub failure_reason: String,
}

/// A runner error plus any delivery metadata accumulated before it failed.
/// This mirrors Go's named `(TaskResult, error)` return without erasing the
/// partially finalized result.
#[derive(Debug)]
pub struct TaskRunFailure {
    pub message: String,
    pub failure_reason: String,
    pub cancelled_delivery_failure: Option<CancelledRunDeliveryFailure>,
}

/// Complete output of one real provider run.
#[derive(Debug)]
pub struct TaskRunOutcome {
    pub result: TaskResult,
    pub failure: Option<TaskRunFailure>,
}

/// Required daemon-core boundary for runtime lookup and provider execution.
///
/// `run_task` owns the established prepare/start/runner/finalize pipeline. It
/// must not return until transcript flushing and local finalization have
/// completed; that ordering is what makes the subsequent cancel ack and
/// terminal callback safe. `ctx` is cancelled when the authoritative server
/// task becomes terminal or disappears.
#[async_trait::async_trait]
pub(crate) trait DaemonTaskExecutionHost: Send + Sync + 'static {
    fn execution_target_for_runtime(&self, runtime_id: &str) -> Option<RuntimeExecutionTarget>;

    /// Preempt low-priority repository maintenance before a task starts, even
    /// when the runner reuses an existing worktree and never checks out.
    async fn cancel_repository_maintenance(&self);

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        target: RuntimeExecutionTarget,
        slot: usize,
    ) -> TaskRunOutcome;
}

#[derive(Debug, Clone)]
pub(crate) struct TaskExecutionConfig {
    pub max_concurrent_tasks: usize,
    pub poll_interval: Duration,
    pub cancel_poll_interval: Duration,
    pub workspaces_root: String,
    pub daemon_id: String,
}

impl TaskExecutionConfig {
    fn cancel_poll_interval(&self) -> Duration {
        if self.cancel_poll_interval.is_zero() {
            DEFAULT_CANCEL_POLL_INTERVAL
        } else {
            self.cancel_poll_interval
        }
    }

    fn capacity_backoff(&self) -> Duration {
        if self.poll_interval.is_zero() || self.poll_interval > TASK_SLOT_CAPACITY_BACKOFF {
            TASK_SLOT_CAPACITY_BACKOFF
        } else {
            self.poll_interval
        }
    }
}

/// Owns the one machine-level batch poller and every claimed task until its
/// terminal delivery attempt finishes.
pub(crate) struct TaskExecutionOrchestrator<H: DaemonTaskExecutionHost> {
    config: TaskExecutionConfig,
    client: Arc<Client>,
    control: Arc<DaemonControl>,
    host: Arc<H>,
    reconcile: Arc<ReconcileBroadcaster>,
    activity: Arc<DaemonActivity>,
}

impl<H: DaemonTaskExecutionHost> TaskExecutionOrchestrator<H> {
    pub(crate) fn new(
        config: TaskExecutionConfig,
        client: Arc<Client>,
        control: Arc<DaemonControl>,
        host: Arc<H>,
        reconcile: Arc<ReconcileBroadcaster>,
        activity: Arc<DaemonActivity>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            config.max_concurrent_tasks > 0,
            "max_concurrent_tasks must be greater than zero"
        );
        Ok(Self {
            config,
            client,
            control,
            host,
            reconcile,
            activity,
        })
    }

    /// Runs until daemon shutdown, then allows in-flight tasks up to 30s to
    /// finish terminal delivery. Dropping a timed-out join handle detaches it;
    /// the process-level shutdown remains authoritative, matching Go's drain.
    pub(crate) async fn run(&self, ctx: Ctx, mut wakeups: mpsc::Receiver<TaskWakeup>) {
        let (slots_tx, mut slots_rx) = mpsc::channel(self.config.max_concurrent_tasks);
        for slot in 0..self.config.max_concurrent_tasks {
            slots_tx
                .try_send(slot)
                .expect("new task slot channel has exact capacity");
        }
        let (nudge_tx, mut nudge_rx) = mpsc::channel::<()>(1);
        let mut runtime_rx = self.control.subscribe_runtime_ids();
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();
        let mut claim_now = true;

        loop {
            if ctx.err().is_some() {
                break;
            }
            if !claim_now {
                tokio::select! {
                    () = ctx.cancelled() => break,
                    changed = runtime_rx.changed() => {
                        if changed.is_err() { break; }
                    }
                    wakeup = wakeups.recv() => {
                        if wakeup.is_none() { break; }
                    }
                    nudge = nudge_rx.recv() => {
                        if nudge.is_none() { break; }
                    }
                    _ = tokio::time::sleep(self.config.poll_interval) => {}
                }
            }
            claim_now = false;

            let runtime_ids = self.control.runtime_ids();
            if runtime_ids.is_empty() {
                continue;
            }

            let Ok(first_slot) = slots_rx.try_recv() else {
                tokio::select! {
                    () = ctx.cancelled() => break,
                    changed = runtime_rx.changed() => {
                        if changed.is_err() { break; }
                        claim_now = true;
                    }
                    wakeup = wakeups.recv() => {
                        if wakeup.is_none() { break; }
                        claim_now = true;
                    }
                    nudge = nudge_rx.recv() => {
                        if nudge.is_none() { break; }
                        claim_now = true;
                    }
                    _ = tokio::time::sleep(self.config.capacity_backoff()) => {}
                }
                continue;
            };
            let mut slots = vec![first_slot];
            while slots.len() < self.config.max_concurrent_tasks {
                let Ok(slot) = slots_rx.try_recv() else {
                    break;
                };
                slots.push(slot);
            }

            let Some(claim_guard) = self.activity.try_enter_claim() else {
                release_slots(&slots_tx, slots);
                continue;
            };

            let claimed = self
                .control
                .claim_tasks(&ctx, &runtime_ids, slots.len())
                .await;
            let claimed = match claimed {
                Ok(claimed) => claimed,
                Err(err) => {
                    tracing::warn!(error = %err, "batch claim failed");
                    release_slots(&slots_tx, slots);
                    continue;
                }
            };

            let claimed_capacity = slots.len();
            let dispatched = claimed.len().min(claimed_capacity);
            if dispatched > 0 {
                // Cache cancellation is a wait-for-exit operation in Rust.
                // Complete it before any provider runner starts so direct Git
                // mutations cannot overlap low-priority maintenance.
                self.host.cancel_repository_maintenance().await;
            }
            let claimed: Vec<Task> = claimed.into_iter().take(dispatched).collect();
            let roots = claimed
                .iter()
                .map(|task| task_env_roots(&self.config.workspaces_root, task))
                .collect();
            // This is the critical claim→active transition: active count and
            // every root are installed before claims_in_flight is released.
            let active_guards = claim_guard.handoff(roots).await;
            for ((task, slot), activity_guard) in claimed
                .into_iter()
                .zip(slots.iter().copied())
                .zip(active_guards)
            {
                tracing::info!(task = %task.id, runtime_id = %task.runtime_id, "task received");
                let client = Arc::clone(&self.client);
                let host = Arc::clone(&self.host);
                let reconcile = Arc::clone(&self.reconcile);
                let task_ctx = ctx.child();
                let slot_release = slots_tx.clone();
                let task_nudge = nudge_tx.clone();
                let cancel_poll_interval = self.config.cancel_poll_interval();
                let daemon_id = self.config.daemon_id.clone();
                tasks.push(tokio::spawn(async move {
                    let _slot_release = TaskSlotRelease::new(slot, slot_release, task_nudge);
                    let _activity_guard = activity_guard;
                    execute_claimed_task(
                        task_ctx,
                        task,
                        slot,
                        client,
                        host,
                        reconcile,
                        cancel_poll_interval,
                        daemon_id,
                    )
                    .await;
                }));
            }
            release_slots(&slots_tx, slots.into_iter().skip(dispatched));

            // A full response is evidence that more work may already be queued.
            claim_now = dispatched > 0 && dispatched == claimed_capacity;
            tasks.retain(|task| !task.is_finished());
        }

        tracing::info!(max_wait = ?SHUTDOWN_DRAIN_TIMEOUT, "task poller stopping; draining tasks");
        let drain = async {
            for task in tasks {
                let _ = task.await;
            }
        };
        if tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, drain)
            .await
            .is_err()
        {
            tracing::warn!("timed out waiting for in-flight tasks");
        }
    }
}

fn task_env_roots(workspaces_root: &str, task: &Task) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(2);
    let predicted = predict_root_dir(workspaces_root, &task.workspace_id, &task.id);
    if !predicted.is_empty() {
        roots.push(PathBuf::from(predicted));
    }
    if !task.prior_work_dir.is_empty() {
        if let Some(prior_root) = Path::new(&task.prior_work_dir).parent() {
            let prior_root = prior_root.to_path_buf();
            if !prior_root.as_os_str().is_empty() && !roots.contains(&prior_root) {
                roots.push(prior_root);
            }
        }
    }
    roots
}

fn release_slots(slots_tx: &mpsc::Sender<usize>, slots: impl IntoIterator<Item = usize>) {
    for slot in slots {
        slots_tx
            .try_send(slot)
            .expect("released slot must fit task slot channel");
    }
}

/// Returns claim capacity even when provider execution or terminal delivery
/// unwinds. The bounded slot channel has exactly one missing entry for every
/// live guard; a full/closed channel means the poller is already shutting down.
struct TaskSlotRelease {
    slot: usize,
    slots: mpsc::Sender<usize>,
    nudge: mpsc::Sender<()>,
}

impl TaskSlotRelease {
    fn new(slot: usize, slots: mpsc::Sender<usize>, nudge: mpsc::Sender<()>) -> Self {
        Self { slot, slots, nudge }
    }
}

impl Drop for TaskSlotRelease {
    fn drop(&mut self) {
        let _ = self.slots.try_send(self.slot);
        let _ = self.nudge.try_send(());
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_claimed_task<H: DaemonTaskExecutionHost>(
    parent_ctx: Ctx,
    task: Task,
    slot: usize,
    client: Arc<Client>,
    host: Arc<H>,
    reconcile: Arc<ReconcileBroadcaster>,
    cancel_poll_interval: Duration,
    daemon_id: String,
) {
    let Some(target) = host.execution_target_for_runtime(&task.runtime_id) else {
        tracing::warn!(
            task = %task.id,
            runtime_id = %task.runtime_id,
            "claimed task targets a runtime this daemon no longer hosts"
        );
        let report_ctx = Ctx::new();
        if let Err(err) = client
            .fail_task(
                &report_ctx,
                &task.id,
                "runtime went offline before the task started",
                "",
                "",
                "",
                patchbay_task_failure::Reason::RUNTIME_OFFLINE.as_str(),
                false,
                "",
                "",
            )
            .await
        {
            tracing::error!(task = %task.id, error = %err, "fail task callback failed");
        }
        return;
    };

    let run_ctx = parent_ctx.child();
    let cancelled_by_server = Arc::new(AtomicBool::new(false));
    let watcher = tokio::spawn(watch_task_cancellation(
        run_ctx.clone(),
        task.id.clone(),
        Arc::clone(&client),
        reconcile,
        cancel_poll_interval,
        Arc::clone(&cancelled_by_server),
    ));
    let outcome = host
        .run_task(run_ctx.clone(), task.clone(), target, slot)
        .await;
    run_ctx.cancel_with(CancelCause::Cancelled);
    let _ = watcher.await;

    if let Err(err) = client
        .report_task_usage(&parent_ctx, &task.id, &outcome.result.usage)
        .await
    {
        tracing::warn!(task = %task.id, error = %err, "report task usage failed");
    }

    // Execution facts are submitted before any terminal callback, using the
    // server-issued daemon capability bound to this claimed runtime. Never
    // put these facts on the task-token lifecycle requests: that credential is
    // intentionally insufficient to author its own provenance.
    record_execution_provenance(&client, &task.execution_daemon_token, &task.id, &outcome.result)
        .await;

    if cancelled_by_server.load(Ordering::Acquire) {
        acknowledge_cancelled_run(&client, &task.id, &outcome).await;
        return;
    }

    if let Some(failure) = &outcome.failure {
        let failure_reason = if failure.failure_reason.is_empty() {
            patchbay_task_failure::classify(&failure.message).to_string()
        } else {
            failure.failure_reason.clone()
        };
        let report_ctx = Ctx::new();
        if let Err(err) = client
            .fail_task(
                &report_ctx,
                &task.id,
                &failure.message,
                &outcome.result.session_id,
                &outcome.result.work_dir,
                &outcome.result.branch_name,
                &failure_reason,
                outcome.result.session_rollout_missing,
                &outcome.result.retired_session_id,
                &outcome.result.durable_work_dir,
            )
            .await
        {
            tracing::error!(task = %task.id, error = %err, "fail task callback failed");
        }
        return;
    }

    if let Err(err) = client
        .report_progress(&parent_ctx, &task.id, "Finishing task", 2, 2)
        .await
    {
        tracing::warn!(task = %task.id, error = %err, "report finishing progress failed");
    }

    let status = client.get_task_status(&parent_ctx, &task.id).await;
    if should_interrupt_agent(&status) {
        acknowledge_cancelled_run(&client, &task.id, &outcome).await;
        return;
    }

    report_task_result(&client, &task.id, &outcome.result).await;
    persist_task_gc_meta(&task, &outcome.result, &daemon_id);
}

/// Classifies a finished task for the persistent GC decision tree. Priority
/// matches Go: chat and autopilot parents outlive issue linkage, while a
/// quick-create task has no issue ID yet and is keyed by task ID.
fn gc_meta_for_task(task: &Task) -> Option<GcMeta> {
    let mut meta = GcMeta {
        workspace_id: task.workspace_id.clone(),
        ..GcMeta::default()
    };
    if !task.chat_session_id.is_empty() {
        meta.kind = Some(GCMetaKind::Chat);
        meta.chat_session_id.clone_from(&task.chat_session_id);
    } else if !task.autopilot_run_id.is_empty() {
        meta.kind = Some(GCMetaKind::AutopilotRun);
        meta.autopilot_run_id.clone_from(&task.autopilot_run_id);
    } else if !task.issue_id.is_empty() {
        meta.kind = Some(GCMetaKind::Issue);
        meta.issue_id.clone_from(&task.issue_id);
    } else if !task.quick_create_prompt.is_empty() {
        meta.kind = Some(GCMetaKind::QuickCreate);
        meta.task_id.clone_from(&task.id);
    } else {
        return None;
    }
    Some(meta)
}

/// Persists GC metadata only after terminal delivery has completed. The outer
/// activity guard remains held by the caller across this write, eliminating
/// the window where GC could see neither an active root nor metadata.
fn persist_task_gc_meta(task: &Task, result: &TaskResult, daemon_id: &str) {
    if result.env_root.is_empty() {
        return;
    }
    let Some(mut meta) = gc_meta_for_task(task) else {
        return;
    };
    match local_directory_assignment_for_task(task, daemon_id) {
        Ok(Some(assignment)) if !assignment.uses_worktree() => meta.local_directory = true,
        Ok(_) => {}
        Err(error) => tracing::warn!(
            task = %task.id,
            %error,
            "could not classify local-directory GC exemption"
        ),
    }
    if let Err(error) = write_gc_meta(&result.env_root, meta) {
        tracing::warn!(task = %task.id, %error, "write gc meta failed (non-fatal)");
    }
}

async fn watch_task_cancellation(
    ctx: Ctx,
    task_id: String,
    client: Arc<Client>,
    reconcile: Arc<ReconcileBroadcaster>,
    poll_interval: Duration,
    cancelled_by_server: Arc<AtomicBool>,
) {
    let mut reconcile_snapshot = reconcile.notify();
    let mut ticker = tokio::time::interval(poll_interval);
    ticker.tick().await;
    loop {
        tokio::select! {
            () = ctx.cancelled() => return,
            () = reconcile_snapshot.recv() => {
                // Resubscribe before I/O so a reconnect overlapping the status
                // request cannot be lost.
                reconcile_snapshot = reconcile.notify();
            }
            _ = ticker.tick() => {}
        }
        let status = client.get_task_status(&ctx, &task_id).await;
        if should_interrupt_agent(&status) {
            match &status {
                Ok(status) => {
                    tracing::info!(task = %task_id, %status, "task terminal server-side; interrupting agent")
                }
                Err(err) => {
                    tracing::info!(task = %task_id, error = %err, "task gone server-side; interrupting agent")
                }
            }
            cancelled_by_server.store(true, Ordering::Release);
            ctx.cancel_with(CancelCause::Cancelled);
            return;
        }
    }
}

fn should_interrupt_agent(status: &anyhow::Result<String>) -> bool {
    match status {
        Ok(status) => matches!(status.as_str(), "completed" | "failed" | "cancelled"),
        Err(err) => is_task_not_found_anyhow(err),
    }
}

async fn acknowledge_cancelled_run(client: &Client, task_id: &str, outcome: &TaskRunOutcome) {
    let mut ack = TaskCancelAck {
        branch_name: outcome.result.branch_name.clone(),
        durable_work_dir: outcome.result.durable_work_dir.clone(),
        session_rollout_missing: outcome.result.session_rollout_missing,
        ..TaskCancelAck::default()
    };
    if let Some(delivery) = outcome
        .failure
        .as_ref()
        .and_then(|failure| failure.cancelled_delivery_failure.as_ref())
    {
        ack.error_message.clone_from(&delivery.error_message);
        ack.failure_reason.clone_from(&delivery.failure_reason);
    }
    let report_ctx = Ctx::new();
    if let Err(err) = client.ack_task_cancelled(&report_ctx, task_id, ack).await {
        tracing::warn!(task = %task_id, error = %err, "cancel ack failed; server sweeper will finalize");
    }
}

/// Refreshes the terminal execution facts through the daemon capability before
/// the task-token lifecycle callback is sent. Missing facts are observable but
/// non-fatal: the server still records an unassociated task outcome.
async fn record_execution_provenance(
    client: &Client,
    daemon_token: &str,
    task_id: &str,
    result: &TaskResult,
) {
    let has_facts = [
        result.execution_repo_identity.trim(),
        result.execution_workspace.trim(),
        result.execution_head_branch.trim(),
        result.execution_head_sha.trim(),
        result.execution_head_state.trim(),
    ]
    .into_iter()
    .any(|value| !value.is_empty());
    if !has_facts {
        return;
    }
    if daemon_token.trim().is_empty() {
        tracing::warn!(
            task_id = %task_id,
            "terminal execution provenance skipped because execution daemon credential is missing"
        );
        return;
    }
    if let Err(error) = client
        .record_execution_provenance(
            &Ctx::new(),
            daemon_token,
            task_id,
            &result.execution_repo_identity,
            &result.execution_workspace,
            &result.execution_head_branch,
            &result.execution_head_sha,
            &result.execution_head_state,
            true,
        )
        .await
    {
        tracing::warn!(%error, task_id = %task_id, "terminal execution provenance refresh failed");
    }
}

async fn report_task_result(client: &Client, task_id: &str, result: &TaskResult) {
    let report_ctx = Ctx::new();
    if result.status == "completed" {
        let complete = client
            .complete_task(
                &report_ctx,
                task_id,
                &result.comment,
                &result.branch_name,
                &result.session_id,
                &result.work_dir,
                result.session_rollout_missing,
                &result.retired_session_id,
                &result.durable_work_dir,
            )
            .await;
        match complete {
            Ok(()) => return,
            Err(err) if is_transient_error(&err) => {
                tracing::error!(task = %task_id, error = %err, "complete task failed after retries; leaving task running");
                return;
            }
            Err(err) => {
                let message = format!("complete task failed: {err}");
                let reason = patchbay_task_failure::classify(&message).to_string();
                tracing::error!(task = %task_id, error = %err, "complete task rejected; falling back to fail");
                if let Err(fail_err) = client
                    .fail_task(
                        &report_ctx,
                        task_id,
                        &message,
                        &result.session_id,
                        &result.work_dir,
                        &result.branch_name,
                        &reason,
                        result.session_rollout_missing,
                        &result.retired_session_id,
                        &result.durable_work_dir,
                    )
                    .await
                {
                    tracing::error!(task = %task_id, error = %fail_err, "fail task fallback also failed");
                }
                return;
            }
        }
    }

    let failure_reason = failure_reason_for_result(result);
    if let Err(err) = client
        .fail_task(
            &report_ctx,
            task_id,
            &result.comment,
            &result.session_id,
            &result.work_dir,
            &result.branch_name,
            &failure_reason,
            result.session_rollout_missing,
            &result.retired_session_id,
            &result.durable_work_dir,
        )
        .await
    {
        tracing::error!(task = %task_id, error = %err, "report failed task failed");
    }
}

fn failure_reason_for_result(result: &TaskResult) -> String {
    if result.failure_reason.is_empty() {
        if result.status == "cancelled" {
            "cancelled".to_string()
        } else {
            patchbay_task_failure::classify(&result.comment).to_string()
        }
    } else {
        result.failure_reason.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::RequestError;

    #[test]
    fn interruption_requires_terminal_status_or_task_not_found() {
        assert!(should_interrupt_agent(&Ok("completed".into())));
        assert!(should_interrupt_agent(&Ok("failed".into())));
        assert!(should_interrupt_agent(&Ok("cancelled".into())));
        assert!(!should_interrupt_agent(&Ok("running".into())));
        assert!(should_interrupt_agent(&Err(anyhow::Error::new(
            RequestError {
                method: "GET",
                path: "/api/daemon/tasks/t/status".into(),
                status_code: 404,
                body: "task not found".into(),
            }
        ))));
        assert!(!should_interrupt_agent(&Err(anyhow::Error::new(
            RequestError {
                method: "GET",
                path: "/api/daemon/tasks/t/status".into(),
                status_code: 503,
                body: "unavailable".into(),
            }
        ))));
    }

    #[test]
    fn zero_cancel_poll_interval_uses_production_default() {
        let config = TaskExecutionConfig {
            max_concurrent_tasks: 1,
            poll_interval: Duration::from_secs(1),
            cancel_poll_interval: Duration::ZERO,
            workspaces_root: "/tmp/workspaces".into(),
            daemon_id: "daemon-1".into(),
        };
        assert_eq!(config.cancel_poll_interval(), Duration::from_secs(5));
    }

    #[test]
    fn task_roots_cover_predicted_and_distinct_prior_environment() {
        let task = Task {
            id: "01a01ec0-e69d-7000-8000-0123456789ab".into(),
            workspace_id: "ws1".into(),
            prior_work_dir: "/old/task/worktree".into(),
            ..Task::default()
        };
        assert_eq!(
            task_env_roots("/workspaces", &task),
            vec![
                PathBuf::from("/workspaces/ws1/0123456789ab"),
                PathBuf::from("/old/task"),
            ]
        );
    }

    #[test]
    fn task_slot_guard_returns_capacity_on_drop() {
        let (slots_tx, mut slots_rx) = mpsc::channel(1);
        let (nudge_tx, mut nudge_rx) = mpsc::channel(1);

        drop(TaskSlotRelease::new(7, slots_tx, nudge_tx));

        assert_eq!(slots_rx.try_recv().unwrap(), 7);
        assert_eq!(nudge_rx.try_recv().unwrap(), ());
    }

    #[test]
    fn terminal_results_fail_closed_and_preserve_explicit_reason() {
        let cancelled = TaskResult {
            status: "cancelled".into(),
            ..TaskResult::default()
        };
        assert_eq!(failure_reason_for_result(&cancelled), "cancelled");

        let blocked = TaskResult {
            status: "blocked".into(),
            comment: "429 provider capacity exhausted".into(),
            ..TaskResult::default()
        };
        assert_eq!(
            failure_reason_for_result(&blocked),
            "agent_error.provider_capacity_or_rate_limit"
        );

        let explicit = TaskResult {
            status: "blocked".into(),
            failure_reason: "skill_bundle_unavailable".into(),
            ..TaskResult::default()
        };
        assert_eq!(
            failure_reason_for_result(&explicit),
            "skill_bundle_unavailable"
        );
    }

    #[tokio::test]
    async fn cancelled_terminal_forwards_missing_rollout_to_cancel_ack() {
        let (request_tx, mut request_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().fallback(axum::routing::any(
            move |request: axum::extract::Request| {
                let request_tx = request_tx.clone();
                async move {
                    let path = request.uri().path().to_string();
                    let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                        .await
                        .unwrap();
                    request_tx
                        .send((
                            path,
                            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                        ))
                        .unwrap();
                    axum::http::StatusCode::NO_CONTENT
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = Client::new(format!("http://{address}"));
        let outcome = TaskRunOutcome {
            result: TaskResult {
                status: "cancelled".to_string(),
                session_rollout_missing: true,
                ..TaskResult::default()
            },
            failure: None,
        };

        acknowledge_cancelled_run(&client, "task-1", &outcome).await;

        let (path, body) = request_rx.recv().await.unwrap();
        assert_eq!(path, "/api/daemon/tasks/task-1/cancel-ack");
        assert_eq!(body["session_rollout_missing"], true);
        server.abort();
    }

    #[test]
    fn gc_meta_classification_preserves_parent_priority() {
        let task = Task {
            id: "task-1".into(),
            workspace_id: "workspace-1".into(),
            issue_id: "issue-1".into(),
            chat_session_id: "chat-1".into(),
            autopilot_run_id: "run-1".into(),
            quick_create_prompt: "prompt".into(),
            ..Task::default()
        };
        let meta = gc_meta_for_task(&task).unwrap();
        assert_eq!(meta.kind, Some(GCMetaKind::Chat));
        assert_eq!(meta.chat_session_id, "chat-1");
        assert!(meta.issue_id.is_empty());

        let quick = Task {
            id: "task-2".into(),
            workspace_id: "workspace-1".into(),
            quick_create_prompt: "prompt".into(),
            ..Task::default()
        };
        let meta = gc_meta_for_task(&quick).unwrap();
        assert_eq!(meta.kind, Some(GCMetaKind::QuickCreate));
        assert_eq!(meta.task_id, "task-2");
    }

    #[test]
    fn writes_terminal_gc_meta_with_local_directory_exemption() {
        let env_root = tempfile::tempdir().unwrap();
        let local_root = tempfile::tempdir().unwrap();
        let task = Task {
            id: "task-1".into(),
            workspace_id: "workspace-1".into(),
            issue_id: "issue-1".into(),
            project_resources: vec![crate::types::ProjectResourceData {
                resource_type: "local_directory".into(),
                resource_ref: serde_json::json!({
                    "local_path": local_root.path(),
                    "daemon_id": "daemon-1",
                    "execution_mode": "in_place"
                }),
                ..crate::types::ProjectResourceData::default()
            }],
            ..Task::default()
        };
        let result = TaskResult {
            env_root: env_root.path().to_string_lossy().into_owned(),
            ..TaskResult::default()
        };

        persist_task_gc_meta(&task, &result, "daemon-1");

        let meta = crate::execenv::execenv::read_gc_meta(&result.env_root).unwrap();
        assert_eq!(meta.kind, Some(GCMetaKind::Issue));
        assert_eq!(meta.issue_id, "issue-1");
        assert_eq!(meta.workspace_id, "workspace-1");
        assert!(meta.local_directory);
        assert!(meta.completed_at.is_some());
    }
}
