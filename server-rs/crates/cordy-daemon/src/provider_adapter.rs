//! Concrete provider execution owned by the production daemon.
//!
//! The outer task orchestrator owns claim/cancel/terminal delivery. This
//! adapter owns the strictly ordered interior: validate and prepare one task
//! environment, transition it to running, launch a real `cordy-agent`
//! backend, persist its bounded transcript, and finalize any disposable local
//! worktree before returning the normalized result.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use cordy_agent::{ExecutionResult, Message, MessageType, Session};
use cordy_protocol::DaemonHeartbeatAckPayload;
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::client::{Client, TaskMessageData};
use crate::config::Config;
use crate::execenv::codex_home::codex_resume_rollout_present;
use crate::execenv::context::{
    cleanup_sidecars, TaskContextMarkerFile, TASK_CONTEXT_MARKER_MANAGED_BY,
    TASK_CONTEXT_MARKER_REL_PATH,
};
use crate::execenv::execenv::{
    predict_root_dir, prepare, read_managed_env_provenance, remove_tree, reuse, Environment,
    MANAGED_ENV_PROVENANCE_MANAGED_BY,
};
use crate::execenv::local_worktree::LocalWorktreeParams;
use crate::execution_plan::{
    PreparedEnvironmentInputs, ProviderExecutionInputs, ProviderExecutionPlan,
};
use crate::health::{ActiveRepoCheckoutTask, HealthResponse};
use crate::local_directory::{
    is_git_work_tree, local_directory_assignment_for_task, validate_local_path,
    LocalDirectoryAssignment, LocalPathLocker, PathLockRelease,
};
use crate::production_services::{ProviderRuntimeAdapter, ProviderRuntimeContext};
use crate::prompt::build_prompt;
use crate::repocache::Ctx;
use crate::runtime_registry::RuntimeRegistry;
use crate::task_execution::{TaskRunFailure, TaskRunOutcome};
use crate::types::{RuntimeExecutionTarget, Task, TaskResult, TaskUsageEntry};

const TRANSCRIPT_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const TRANSCRIPT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const TRANSCRIPT_DRAIN_GRACE: Duration = Duration::from_secs(10);
const TRANSCRIPT_BATCH_LIMIT: usize = 32;
const TOOL_OUTPUT_BYTES: usize = 8 * 1024;
const TOOL_INPUT_BYTES: usize = 64 * 1024;
const PREPARE_LEASE_REFRESH: Duration = Duration::from_secs(15);
const PREPARE_LEASE_TIMEOUT: Duration = Duration::from_secs(10);
const PREPARE_LEASE_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const CODEX_ROLLOUT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Real provider adapter for protocol families implemented by `cordy-agent`.
/// Metadata-only runtimes fail at `build_backend`; no provider can turn into a
/// pretend success path.
pub struct ProductionProviderAdapter {
    config: Arc<Config>,
    local_paths: Arc<LocalPathLocker>,
    started_at: Instant,
    active_tasks: AtomicI64,
    running_tasks: AtomicI64,
    resource_wait_tasks: AtomicI64,
}

impl ProductionProviderAdapter {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            local_paths: Arc::new(LocalPathLocker::new()),
            started_at: Instant::now(),
            active_tasks: AtomicI64::new(0),
            running_tasks: AtomicI64::new(0),
            resource_wait_tasks: AtomicI64::new(0),
        }
    }

    async fn run_task_inner(
        &self,
        ctx: Ctx,
        mut task: Task,
        target: RuntimeExecutionTarget,
        slot: usize,
        runtime: ProviderRuntimeContext,
    ) -> TaskRunOutcome {
        let _active = CounterGuard::new(&self.active_tasks);
        let assignment = match local_directory_assignment_for_task(&task, &self.config.daemon_id) {
            Ok(assignment) => assignment,
            Err(error) => return failed(error, None),
        };
        if let Some(assignment) = &assignment {
            if let Err(error) = assignment.validate_execution_mode() {
                return failed(error, None);
            }
            if let Err(error) = validate_local_path(&assignment.abs_path) {
                return failed(error, None);
            }
            if assignment.uses_worktree() && !is_git_work_tree(&ctx, &assignment.abs_path).await {
                return failed(
                    anyhow::anyhow!(
                        "local_directory: worktree mode requires a git working tree: {:?}",
                        assignment.abs_path
                    ),
                    None,
                );
            }
        }

        let predicted_root =
            predict_root_dir(&self.config.workspaces_root, &task.workspace_id, &task.id);
        let temp_dir = Path::new(&predicted_root)
            .join("tmp")
            .to_string_lossy()
            .into_owned();
        let launch = runtime
            .launch_registry()
            .resolve(&task.workspace_id, &target);
        let Some(launch) = launch else {
            return failed(
                anyhow::anyhow!(
                    "no accepted launch registered for workspace {:?} and provider {}",
                    task.workspace_id,
                    target.provider
                ),
                None,
            );
        };
        let default_model = self
            .config
            .agents
            .get(&target.provider)
            .map(|entry| entry.model.clone())
            .unwrap_or_default();
        let mut inputs = ProviderExecutionInputs {
            slot,
            temp_dir: temp_dir.clone(),
            default_model,
            codex_version: launch.version.clone(),
            openclaw_bin: (target.provider == "openclaw")
                .then(|| launch.command_path.clone())
                .unwrap_or_default(),
            launch_prefix: launch.fixed_args.clone(),
            path: provider_path(),
            ..ProviderExecutionInputs::default()
        };
        if let Some(agent) = task.agent.as_ref() {
            inputs.cursor_mcp_auth_source = agent
                .custom_env
                .as_ref()
                .and_then(|env| env.get("CURSOR_MCP_AUTH_SOURCE"))
                .cloned()
                .unwrap_or_default();
        }
        if let Some(assignment) = &assignment {
            if assignment.uses_worktree() {
                inputs.local_worktree = Some(LocalWorktreeParams {
                    local_path: assignment.abs_path.clone(),
                    ..LocalWorktreeParams::default()
                });
            } else {
                inputs.local_work_dir = assignment.abs_path.clone();
            }
        }
        let mut plan = match ProviderExecutionPlan::build(&self.config, &task, &target, inputs) {
            Ok(plan) => plan,
            Err(error) => return failed(error, None),
        };
        let requested_session_id = plan.resume_session_id().to_string();

        let client = runtime.client();
        let prepare_lease = PrepareLeaseExtender::start(
            ctx.clone(),
            Arc::clone(&client),
            task.runtime_id.clone(),
            task.id.clone(),
        );
        let path_guard = match self
            .acquire_local_path(&ctx, &client, &task, assignment.as_ref())
            .await
        {
            Ok(guard) => guard,
            Err(error) => return failed(error, None),
        };
        let (mut environment, resumed) = match self
            .prepare_environment(&ctx, &task, &mut plan, assignment.as_ref(), &path_guard)
            .await
        {
            Ok(environment) => environment,
            Err(error) => return failed(error, None),
        };
        // Worktree mode holds the source-path lock only while taking its
        // consistent snapshot. In-place mode deliberately retains it until
        // the complete result has been finalized.
        let path_guard = if assignment
            .as_ref()
            .is_some_and(LocalDirectoryAssignment::uses_worktree)
        {
            drop(path_guard);
            None
        } else {
            path_guard
        };
        prepare_lease.stop().await;

        if let Err(error) = std::fs::create_dir_all(&temp_dir) {
            let outcome = failed(
                anyhow::Error::new(error).context("create task temp directory"),
                Some(&environment),
            );
            return finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
        }
        let run = async {
            client
                .start_task(&ctx, &task.id)
                .await
                .map_err(|error| anyhow::anyhow!("start task failed: {error}"))?;
            if let Err(error) = client
                .report_progress(
                    &ctx,
                    &task.id,
                    &format!("Launching {}", target.provider),
                    1,
                    2,
                )
                .await
            {
                tracing::debug!(task = %task.id, %error, "report launching progress failed");
            }

            // A prior provider session is authoritative only together with a
            // reusable daemon-owned workdir. Starting fresh while forwarding
            // an unrelated session id is a cross-task state leak.
            if !resumed && !task.prior_session_id.is_empty() {
                task.prior_session_id.clear();
                task.prior_session_resume_unavailable = true;
            }
            let bound = plan.bind_environment(
                &environment,
                PreparedEnvironmentInputs {
                    cancellation: ctx.token().clone(),
                    openclaw_include_roots: environment.openclaw_include_root.clone(),
                    ..PreparedEnvironmentInputs::default()
                },
            )?;
            let backend_config = runtime.backend_config_with_prefix(
                &task.workspace_id,
                &target,
                bound.child_env.into_inner(),
                bound.launch_prefix,
            )?;
            let backend = cordy_agent::build_backend(&target.provider, backend_config)
                .map_err(|error| anyhow::anyhow!("create agent backend: {error}"))?;
            let token = task.auth_token.trim().to_string();
            let _checkout = runtime.checkout_registry().register_owned(
                token,
                ActiveRepoCheckoutTask {
                    workspace_id: task.workspace_id.clone(),
                    task_id: task.id.clone(),
                    agent_id: task.agent_id.clone(),
                    agent_name: task
                        .agent
                        .as_ref()
                        .map(|agent| agent.name.clone())
                        .unwrap_or_default(),
                    work_dir: environment.work_dir.clone(),
                },
            );
            let prompt = build_prompt(task.clone(), &target.provider);
            let session = backend
                .execute(&prompt, bound.options.clone())
                .await
                .map_err(|error| anyhow::anyhow!("execute {}: {error}", target.provider))?;
            let _running = CounterGuard::new(&self.running_tasks);
            let mut transcript_seq = 0;
            let (mut result, tools_seen) = drain_session(
                &ctx,
                &client,
                &task.id,
                &environment.work_dir,
                &environment.codex_home,
                session,
                &mut transcript_seq,
            )
            .await?;
            let mut retired_session_id = String::new();
            if target.provider == "hermes"
                && should_retry_with_fresh_session(&result, &requested_session_id, tools_seen)
            {
                retired_session_id.clone_from(&requested_session_id);
                task.prior_session_id.clear();
                task.prior_session_resume_unavailable = true;
                plan.drop_resume();
                let mut fresh_options = bound.options.clone();
                fresh_options.resume_session_id.clear();
                fresh_options.resume_expected = false;
                fresh_options.resume_continuity_notice.clear();
                let fresh_prompt = build_prompt(task.clone(), &target.provider);
                match backend.execute(&fresh_prompt, fresh_options).await {
                    Ok(fresh_session) => match drain_session(
                        &ctx,
                        &client,
                        &task.id,
                        &environment.work_dir,
                        &environment.codex_home,
                        fresh_session,
                        &mut transcript_seq,
                    )
                    .await
                    {
                        Ok((fresh_result, _))
                            if fresh_result.status == "completed"
                                || !fresh_result.session_id.is_empty() =>
                        {
                            result = fresh_result;
                        }
                        Ok((fresh_result, _)) => {
                            tracing::warn!(
                                task = %task.id,
                                error = %fresh_result.error,
                                "fresh Hermes session retry did not establish a session; keeping the original result"
                            );
                        }
                        Err(error) => tracing::warn!(
                            task = %task.id,
                            %error,
                            "fresh Hermes session retry failed; keeping the original result"
                        ),
                    },
                    Err(error) => tracing::warn!(
                        task = %task.id,
                        %error,
                        "fresh Hermes session retry could not start; keeping the original result"
                    ),
                }
            }
            Ok((result, retired_session_id))
        }
        .await;

        let mut outcome = match run {
            Ok((result, retired_session_id)) => result_outcome(
                &target.provider,
                result,
                &environment,
                &requested_session_id,
                &retired_session_id,
            ),
            Err(error) => failed(error, Some(&environment)),
        };
        if let Err(error) = remove_tree(&temp_dir) {
            tracing::warn!(task = %task.id, %error, "task temp directory cleanup failed");
        }
        outcome = finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
        drop(path_guard);
        outcome
    }

    async fn acquire_local_path(
        &self,
        ctx: &Ctx,
        client: &Client,
        task: &Task,
        assignment: Option<&LocalDirectoryAssignment>,
    ) -> anyhow::Result<Option<PathLockRelease>> {
        let Some(assignment) = assignment else {
            return Ok(None);
        };
        let holder = self.local_paths.holder(&assignment.real_path);
        let waiting = !holder.is_empty();
        if waiting {
            self.resource_wait_tasks.fetch_add(1, Ordering::AcqRel);
            let reason = format!("waiting for local directory held by task {holder}");
            if let Err(error) = client
                .mark_task_waiting_local_directory(ctx, &task.id, &reason)
                .await
            {
                tracing::warn!(task = %task.id, %error, "mark task waiting for local directory failed");
            }
        }
        let acquired = self
            .local_paths
            .acquire(ctx, &assignment.real_path, &task.id, None)
            .await;
        if waiting {
            self.resource_wait_tasks.fetch_sub(1, Ordering::AcqRel);
        }
        acquired.map(Some)
    }

    async fn prepare_environment(
        &self,
        ctx: &Ctx,
        task: &Task,
        plan: &mut ProviderExecutionPlan,
        assignment: Option<&LocalDirectoryAssignment>,
        _path_guard: &Option<PathLockRelease>,
    ) -> anyhow::Result<(Environment, bool)> {
        if ctx.err().is_some() {
            anyhow::bail!(ctx.cause().to_string());
        }
        if assignment.is_none() && reusable_workdir(&self.config.workspaces_root, task) {
            if let Some(environment) = reuse(plan.reuse_params(task.prior_work_dir.clone())) {
                return Ok((environment, true));
            }
        }
        plan.drop_resume();
        tokio::select! {
            result = prepare(plan.prepare_params()) => result
                .map(|environment| (environment, false))
                .map_err(|error| anyhow::anyhow!("prepare execution environment: {error:#}")),
            () = ctx.cancelled() => Err(anyhow::anyhow!(ctx.cause().to_string())),
        }
    }
}

#[async_trait::async_trait]
impl ProviderRuntimeAdapter for ProductionProviderAdapter {
    async fn handle_non_update_heartbeat_actions(
        &self,
        _ctx: Ctx,
        _registry: Arc<RuntimeRegistry>,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
    ) {
        if ack.pending_model_list.is_some()
            || ack.pending_local_skills.is_some()
            || ack.pending_local_skill_import.is_some()
            || !ack.pending_local_skill_imports.is_empty()
        {
            tracing::error!(
                %runtime_id,
                "heartbeat action unsupported by production provider adapter; request left unacknowledged"
            );
        }
    }

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        target: RuntimeExecutionTarget,
        slot: usize,
        runtime: ProviderRuntimeContext,
    ) -> TaskRunOutcome {
        self.run_task_inner(ctx, task, target, slot, runtime).await
    }

    fn health_snapshot(&self) -> HealthResponse {
        HealthResponse {
            status: "ok".to_string(),
            pid: std::process::id() as i32,
            os: std::env::consts::OS.to_string(),
            uptime: format!("{:?}", self.started_at.elapsed()),
            daemon_id: self.config.daemon_id.clone(),
            device_name: self.config.device_name.clone(),
            server_url: self.config.server_base_url.clone(),
            cli_version: self.config.cli_version.clone(),
            launched_by: self.config.launched_by.clone(),
            active_task_count: self.active_tasks.load(Ordering::Acquire),
            running_task_count: self.running_tasks.load(Ordering::Acquire),
            resource_wait_task_count: self.resource_wait_tasks.load(Ordering::Acquire),
            agents: self.config.agents.keys().cloned().collect(),
            ..HealthResponse::default()
        }
    }
}

struct CounterGuard<'a> {
    counter: &'a AtomicI64,
}

impl<'a> CounterGuard<'a> {
    fn new(counter: &'a AtomicI64) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for CounterGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Extends the server's dispatched-task lease only during the potentially
/// slow local lock and filesystem preparation phase. The worker is owned:
/// normal stop joins it, while every early return aborts it in `Drop`.
struct PrepareLeaseExtender {
    stop: CancellationToken,
    worker: Option<JoinHandle<()>>,
}

impl PrepareLeaseExtender {
    fn start(ctx: Ctx, client: Arc<Client>, runtime_id: String, task_id: String) -> Self {
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let worker = tokio::spawn(async move {
            let start = tokio::time::Instant::now() + PREPARE_LEASE_REFRESH;
            let mut ticker = tokio::time::interval_at(start, PREPARE_LEASE_REFRESH);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = ctx.cancelled() => break,
                    () = worker_stop.cancelled() => break,
                    _ = ticker.tick() => {
                        let request_ctx = ctx.child();
                        let request = client.extend_task_prepare_lease(
                            &request_ctx,
                            &runtime_id,
                            &task_id,
                        );
                        tokio::select! {
                            () = ctx.cancelled() => break,
                            () = worker_stop.cancelled() => break,
                            result = tokio::time::timeout(PREPARE_LEASE_TIMEOUT, request) => {
                                match result {
                                    Ok(Ok(())) => {}
                                    Ok(Err(error)) => tracing::warn!(task = %task_id, %error, "extend task prepare lease failed"),
                                    Err(_) => tracing::warn!(task = %task_id, "extend task prepare lease timed out"),
                                }
                            }
                        }
                    }
                }
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }

    async fn stop(mut self) {
        self.stop.cancel();
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        if tokio::time::timeout(PREPARE_LEASE_STOP_TIMEOUT, &mut worker)
            .await
            .is_err()
        {
            worker.abort();
            let _ = worker.await;
        }
    }
}

impl Drop for PrepareLeaseExtender {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

async fn drain_session(
    ctx: &Ctx,
    client: &Client,
    task_id: &str,
    work_dir: &str,
    codex_home: &str,
    session: Session,
    next_seq: &mut i32,
) -> anyhow::Result<(ExecutionResult, usize)> {
    let Session {
        mut messages,
        mut result,
    } = session;
    let mut ticker = tokio::time::interval(TRANSCRIPT_FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut transcript = TranscriptBatch {
        next_seq: *next_seq,
        ..TranscriptBatch::default()
    };
    let mut terminal: Option<ExecutionResult> = None;
    let mut tools_seen = 0usize;
    let mut cancelled = false;
    let mut messages_closed = false;
    let mut result_closed = false;
    let mut pending_session_id: Option<String> = None;
    let mut drain_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(365 * 24 * 3600)));
    let mut drain_armed = false;
    let mut rollout_ticker = tokio::time::interval(CODEX_ROLLOUT_POLL_INTERVAL);
    rollout_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if messages_closed && terminal.is_some() {
            break;
        }
        tokio::select! {
            () = ctx.cancelled(), if terminal.is_none() => {
                cancelled = true;
                terminal = Some(ExecutionResult {
                    status: "cancelled".to_string(),
                    error: ctx.cause().to_string(),
                    ..ExecutionResult::default()
                });
                drain_deadline.as_mut().reset(tokio::time::Instant::now() + TRANSCRIPT_DRAIN_GRACE);
                drain_armed = true;
            }
            received = messages.recv(), if !messages_closed => {
                match received {
                    Some(message) => {
                        if message.message_type == MessageType::ToolUse {
                            tools_seen = tools_seen.saturating_add(1);
                        }
                        if let Some(session_id) = transcript.push(message) {
                            pending_session_id = Some(session_id);
                            pin_session_if_ready(client, task_id, work_dir, codex_home, &mut pending_session_id).await;
                        }
                        if transcript.ready() {
                            flush_transcript(client, task_id, &mut transcript).await;
                        }
                    }
                    None => messages_closed = true,
                }
            }
            received = &mut result, if !result_closed => {
                result_closed = true;
                if !cancelled {
                    match received {
                        Ok(value) => terminal = Some(value),
                        Err(error) => {
                            terminal = Some(ExecutionResult {
                                status: "failed".to_string(),
                                error: format!("provider result channel closed: {error}"),
                                ..ExecutionResult::default()
                            });
                        }
                    }
                }
                drain_deadline.as_mut().reset(tokio::time::Instant::now() + TRANSCRIPT_DRAIN_GRACE);
                drain_armed = true;
            }
            _ = ticker.tick() => flush_transcript(client, task_id, &mut transcript).await,
            _ = rollout_ticker.tick() => {
                pin_session_if_ready(
                    client,
                    task_id,
                    work_dir,
                    codex_home,
                    &mut pending_session_id,
                )
                .await;
            }
            () = &mut drain_deadline, if drain_armed => {
                tracing::warn!(task = %task_id, "provider transcript did not close within drain grace");
                break;
            }
        }
    }
    pin_session_if_ready(
        client,
        task_id,
        work_dir,
        codex_home,
        &mut pending_session_id,
    )
    .await;
    flush_transcript(client, task_id, &mut transcript).await;
    *next_seq = transcript.next_seq;
    Ok((
        terminal.unwrap_or_else(|| ExecutionResult {
            status: "failed".to_string(),
            error: "provider messages closed without a terminal result".to_string(),
            ..ExecutionResult::default()
        }),
        tools_seen,
    ))
}

fn session_pin_ready(codex_home: &str, session_id: &str) -> bool {
    !session_id.is_empty()
        && (codex_home.is_empty() || codex_resume_rollout_present(codex_home, session_id))
}

async fn pin_session_if_ready(
    client: &Client,
    task_id: &str,
    work_dir: &str,
    codex_home: &str,
    pending_session_id: &mut Option<String>,
) {
    let Some(session_id) = pending_session_id.as_deref() else {
        return;
    };
    if !session_pin_ready(codex_home, session_id) {
        return;
    }
    let session_id = pending_session_id.take().unwrap_or_default();
    let pin_ctx = Ctx::new();
    let pin = client.pin_task_session(&pin_ctx, task_id, &session_id, work_dir);
    match tokio::time::timeout(TRANSCRIPT_REQUEST_TIMEOUT, pin).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(task = %task_id, %error, "pin task session failed"),
        Err(_) => tracing::debug!(task = %task_id, "pin task session timed out"),
    }
}

#[derive(Default)]
struct TranscriptBatch {
    next_seq: i32,
    messages: Vec<TaskMessageData>,
    tools: HashMap<String, String>,
    session_pinned: bool,
}

impl TranscriptBatch {
    fn ready(&self) -> bool {
        self.messages.len() >= TRANSCRIPT_BATCH_LIMIT
    }

    fn push(&mut self, message: Message) -> Option<String> {
        if message.message_type == MessageType::Status {
            if !self.session_pinned && !message.session_id.is_empty() {
                self.session_pinned = true;
                return Some(message.session_id);
            }
            return None;
        }
        if matches!(message.message_type, MessageType::Log) {
            return None;
        }
        self.next_seq = self.next_seq.saturating_add(1);
        let mut value = TaskMessageData {
            seq: self.next_seq,
            r#type: message_type_name(message.message_type).to_string(),
            tool: message.tool.clone(),
            content: bounded_text(&message.content, TOOL_OUTPUT_BYTES),
            output: bounded_text(&message.output, TOOL_OUTPUT_BYTES),
            ..TaskMessageData::default()
        };
        if message.message_type == MessageType::ToolUse {
            if !message.call_id.is_empty() {
                self.tools
                    .insert(message.call_id.clone(), message.tool.clone());
            }
            value.input = Some(redact_tool_input(message.input));
        } else if message.message_type == MessageType::ToolResult
            && value.tool.is_empty()
            && !message.call_id.is_empty()
        {
            value.tool = self
                .tools
                .get(&message.call_id)
                .cloned()
                .unwrap_or_default();
        }
        self.messages.push(value);
        None
    }
}

async fn flush_transcript(client: &Client, task_id: &str, batch: &mut TranscriptBatch) {
    if batch.messages.is_empty() {
        return;
    }
    let messages = std::mem::take(&mut batch.messages);
    let ctx = Ctx::new();
    let report = client.report_task_messages(&ctx, task_id, messages);
    match tokio::time::timeout(TRANSCRIPT_REQUEST_TIMEOUT, report).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(task = %task_id, %error, "report task messages failed"),
        Err(_) => tracing::debug!(task = %task_id, "report task messages timed out"),
    }
}

fn message_type_name(message_type: MessageType) -> &'static str {
    match message_type {
        MessageType::Text => "text",
        MessageType::Thinking => "thinking",
        MessageType::ToolUse => "tool_use",
        MessageType::ToolResult => "tool_result",
        MessageType::Error => "error",
        MessageType::Status => "status",
        MessageType::Log => "log",
    }
}

fn redact_tool_input(input: BTreeMap<String, Value>) -> Map<String, Value> {
    let mut value = Value::Object(input.into_iter().collect());
    if serde_json::to_vec(&value).map_or(true, |encoded| encoded.len() > TOOL_INPUT_BYTES) {
        return Map::from_iter([(
            "_".to_string(),
            Value::String("[REDACTED OVERSIZED INPUT]".to_string()),
        )]);
    }
    redact_value(&mut value, 0);
    value.as_object().cloned().unwrap_or_default()
}

fn redact_value(value: &mut Value, depth: usize) {
    if depth >= 32 {
        *value = Value::String("[REDACTED DEPTH LIMIT]".to_string());
        return;
    }
    match value {
        Value::String(text) => *text = cordy_agent::stderr::sanitize_diagnostic(text),
        Value::Array(values) => {
            for value in values {
                redact_value(value, depth + 1);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_value(value, depth + 1);
            }
        }
        _ => {}
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn should_retry_with_fresh_session(
    result: &ExecutionResult,
    requested_session_id: &str,
    tools_seen: usize,
) -> bool {
    result.status == "failed"
        && result.resume_rejected
        && !requested_session_id.is_empty()
        && tools_seen == 0
}

fn result_outcome(
    provider: &str,
    result: ExecutionResult,
    env: &Environment,
    requested_session_id: &str,
    retired_session_id: &str,
) -> TaskRunOutcome {
    let resume_rejected = result.resume_rejected && !requested_session_id.is_empty();
    let mut usage = result
        .usage
        .into_iter()
        .filter_map(|(model, value)| {
            (value.input_tokens != 0
                || value.output_tokens != 0
                || value.cache_read_tokens != 0
                || value.cache_write_tokens != 0
                || value.cost_usd_ticks != 0)
                .then_some(TaskUsageEntry {
                    provider: provider.to_string(),
                    model,
                    input_tokens: value.input_tokens,
                    output_tokens: value.output_tokens,
                    cache_read_tokens: value.cache_read_tokens,
                    cache_write_tokens: value.cache_write_tokens,
                    cost_usd_ticks: value.cost_usd_ticks,
                })
        })
        .collect::<Vec<_>>();
    usage.sort_by(|left, right| left.model.cmp(&right.model));
    let (status, comment, mut failure_reason) = match result.status.as_str() {
        "completed" => ("completed", result.output, String::new()),
        "cancelled" => (
            "cancelled",
            if result.error.is_empty() {
                "task cancelled by server".to_string()
            } else {
                result.error
            },
            "cancelled".to_string(),
        ),
        "timeout" => (
            "blocked",
            if result.error.is_empty() {
                format!("{provider} timed out")
            } else {
                result.error
            },
            "timeout".to_string(),
        ),
        "idle_watchdog" => ("blocked", result.error, "idle_watchdog".to_string()),
        _ => (
            "blocked",
            if result.error.is_empty() {
                format!("{provider} execution {}", result.status)
            } else {
                result.error
            },
            String::new(),
        ),
    };
    if resume_rejected {
        failure_reason = "resume_rejected".to_string();
    }
    TaskRunOutcome {
        result: TaskResult {
            status: status.to_string(),
            comment,
            session_id: result.session_id,
            work_dir: env.work_dir.clone(),
            env_root: env.root_dir.clone(),
            failure_reason,
            retired_session_id: if resume_rejected {
                requested_session_id.to_string()
            } else {
                retired_session_id.to_string()
            },
            usage,
            ..TaskResult::default()
        },
        failure: None,
    }
}

fn failed(error: anyhow::Error, environment: Option<&Environment>) -> TaskRunOutcome {
    let mut result = TaskResult::default();
    if let Some(environment) = environment {
        result.work_dir = environment.work_dir.clone();
        result.env_root = environment.root_dir.clone();
    }
    TaskRunOutcome {
        result,
        failure: Some(TaskRunFailure {
            message: format!("{error:#}"),
            failure_reason: String::new(),
            cancelled_delivery_failure: None,
        }),
    }
}

async fn finalize_environment(
    mut outcome: TaskRunOutcome,
    environment: &mut Environment,
    assignment: Option<&LocalDirectoryAssignment>,
) -> TaskRunOutcome {
    if environment.local_directory || environment.local_worktree.is_some() {
        if let Err(error) = cleanup_sidecars(&environment.root_dir) {
            tracing::warn!(%error, "execenv: cleanup sidecars failed");
            if let Some(worktree) = environment.local_worktree.as_mut() {
                worktree.abort_with_reason(&anyhow::anyhow!(
                    "could not remove daemon sidecars before worktree delivery: {error}"
                ));
            }
        }
    }
    if let Some(worktree) = environment.local_worktree.as_ref() {
        match worktree.finalize().await {
            Ok(finalized) => {
                outcome.result.branch_name = finalized.branch;
                if let Some(assignment) = assignment {
                    outcome.result.durable_work_dir = assignment.abs_path.clone();
                }
            }
            Err(error) => {
                let finalize_message = format!("local_directory worktree finalize: {error:#}");
                let message = outcome.failure.as_ref().map_or_else(
                    || finalize_message.clone(),
                    |failure| format!("{}; {finalize_message}", failure.message),
                );
                outcome.failure = Some(TaskRunFailure {
                    message: message.clone(),
                    failure_reason: String::new(),
                    cancelled_delivery_failure: Some(
                        crate::task_execution::CancelledRunDeliveryFailure {
                            error_message: message,
                            failure_reason: "agent_error".to_string(),
                        },
                    ),
                });
            }
        }
    }
    outcome
}

fn reusable_workdir(workspaces_root: &str, task: &Task) -> bool {
    if task.prior_work_dir.is_empty()
        || task.agent_id.is_empty()
        || (task.issue_id.is_empty() && task.chat_session_id.is_empty())
    {
        return false;
    }
    let Ok(root) = std::fs::canonicalize(workspaces_root) else {
        return false;
    };
    let Ok(workdir) = std::fs::canonicalize(&task.prior_work_dir) else {
        return false;
    };
    if !workdir.is_dir() {
        return false;
    }
    let Ok(relative) = workdir.strip_prefix(&root) else {
        return false;
    };
    let parts = relative.components().collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0].as_os_str() != std::ffi::OsStr::new(&task.workspace_id)
        || parts[2].as_os_str() != std::ffi::OsStr::new("workdir")
    {
        return false;
    }
    let env_root = workdir.parent().unwrap_or(Path::new(""));
    let Ok(provenance) = read_managed_env_provenance(&env_root.to_string_lossy()) else {
        return false;
    };
    if provenance.managed_by != MANAGED_ENV_PROVENANCE_MANAGED_BY
        || provenance.workspace_id != task.workspace_id
        || provenance.agent_id != task.agent_id
    {
        return false;
    }
    let marker_path = workdir.join(TASK_CONTEXT_MARKER_REL_PATH);
    let Ok(marker_bytes) = std::fs::read(marker_path) else {
        return false;
    };
    let Ok(marker) = serde_json::from_slice::<TaskContextMarkerFile>(&marker_bytes) else {
        return false;
    };
    marker.managed_by == TASK_CONTEXT_MARKER_MANAGED_BY
        && marker.agent_id == task.agent_id
        && if !task.issue_id.is_empty() {
            provenance.issue_id == task.issue_id && marker.issue_id == task.issue_id
        } else {
            provenance.chat_session_id == task.chat_session_id
                && marker.chat_session_id == task.chat_session_id
        }
}

fn provider_path() -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    let Ok(executable) = std::env::current_exe() else {
        return inherited;
    };
    let Some(parent) = executable.parent() else {
        return inherited;
    };
    if inherited.is_empty() {
        parent.to_string_lossy().into_owned()
    } else {
        std::env::join_paths(
            std::iter::once(parent.to_path_buf()).chain(std::env::split_paths(&inherited)),
        )
        .ok()
        .and_then(|path| path.into_string().ok())
        .unwrap_or(inherited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordy_agent::TokenUsage;

    #[test]
    fn tool_input_is_redacted_before_it_becomes_transcript_data() {
        let input = BTreeMap::from([
            (
                "authorization".to_string(),
                Value::String("Bearer secret-token".to_string()),
            ),
            (
                "nested".to_string(),
                serde_json::json!({"token":"sk-abcdefghijklmnopqrstuvwxyz"}),
            ),
        ]);
        let redacted = redact_tool_input(input);
        assert_ne!(redacted["authorization"], "Bearer secret-token");
        assert_ne!(redacted["nested"]["token"], "sk-abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn transcript_sequence_and_tool_name_are_daemon_authoritative() {
        let mut batch = TranscriptBatch::default();
        batch.push(Message {
            message_type: MessageType::ToolUse,
            content: String::new(),
            tool: "read_file".to_string(),
            call_id: "call-1".to_string(),
            input: BTreeMap::new(),
            output: String::new(),
            status: String::new(),
            level: String::new(),
            session_id: String::new(),
        });
        batch.push(Message {
            message_type: MessageType::ToolResult,
            content: String::new(),
            tool: String::new(),
            call_id: "call-1".to_string(),
            input: BTreeMap::new(),
            output: "ok".to_string(),
            status: String::new(),
            level: String::new(),
            session_id: String::new(),
        });
        assert_eq!(batch.messages[0].seq, 1);
        assert_eq!(batch.messages[1].seq, 2);
        assert_eq!(batch.messages[1].tool, "read_file");
    }

    #[test]
    fn hermes_resume_retry_requires_a_toolless_rejected_attempt() {
        let rejected = ExecutionResult {
            status: "failed".to_string(),
            resume_rejected: true,
            ..ExecutionResult::default()
        };
        assert!(should_retry_with_fresh_session(&rejected, "old-session", 0));
        assert!(!should_retry_with_fresh_session(
            &rejected,
            "old-session",
            1
        ));
        assert!(!should_retry_with_fresh_session(&rejected, "", 0));
        assert!(!should_retry_with_fresh_session(
            &ExecutionResult {
                status: "completed".to_string(),
                resume_rejected: true,
                ..ExecutionResult::default()
            },
            "old-session",
            0,
        ));
    }

    #[test]
    fn provider_result_maps_usage_and_non_success_statuses() {
        let result = ExecutionResult {
            status: "timeout".to_string(),
            error: "provider timed out".to_string(),
            session_id: "session-1".to_string(),
            usage: BTreeMap::from([(
                "model-1".to_string(),
                TokenUsage {
                    input_tokens: 10,
                    output_tokens: 3,
                    ..TokenUsage::default()
                },
            )]),
            ..ExecutionResult::default()
        };
        let outcome = result_outcome(
            "qwen",
            result,
            &Environment {
                work_dir: "/work".to_string(),
                root_dir: "/root".to_string(),
                ..Environment::default()
            },
            "session-old",
            "",
        );
        assert_eq!(outcome.result.status, "blocked");
        assert_eq!(outcome.result.failure_reason, "timeout");
        assert_eq!(outcome.result.session_id, "session-1");
        assert_eq!(outcome.result.usage[0].provider, "qwen");
        assert_eq!(outcome.result.usage[0].input_tokens, 10);
    }

    #[test]
    fn cancellation_is_a_terminal_result_not_a_transport_failure() {
        let outcome = result_outcome(
            "qwen",
            ExecutionResult {
                status: "cancelled".to_string(),
                ..ExecutionResult::default()
            },
            &Environment::default(),
            "",
            "",
        );
        assert!(outcome.failure.is_none());
        assert_eq!(outcome.result.status, "cancelled");
        assert_eq!(outcome.result.failure_reason, "cancelled");
    }

    #[test]
    fn rejected_resume_is_retired_fail_closed() {
        let outcome = result_outcome(
            "qwen",
            ExecutionResult {
                status: "failed".to_string(),
                error: "resume unavailable".to_string(),
                resume_rejected: true,
                ..ExecutionResult::default()
            },
            &Environment::default(),
            "session-poisoned",
            "",
        );
        assert_eq!(outcome.result.failure_reason, "resume_rejected");
        assert_eq!(outcome.result.retired_session_id, "session-poisoned");
    }

    #[test]
    fn codex_session_pin_requires_a_persisted_rollout() {
        assert!(session_pin_ready("", "provider-session"));
        assert!(!session_pin_ready(
            "/missing-codex-home",
            "provider-session"
        ));

        let home = tempfile::tempdir().unwrap();
        let rollout = home
            .path()
            .join("sessions/2026/08/24/rollout-2026-08-24T00-00-00-provider-session.jsonl");
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        std::fs::write(&rollout, b"{}\n").unwrap();
        assert!(session_pin_ready(
            home.path().to_str().unwrap(),
            "provider-session"
        ));
    }
}
