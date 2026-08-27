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

use cordy_agent::{
    Backend, CatalogCache, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use cordy_protocol::DaemonHeartbeatAckPayload;
use serde_json::{json, Map, Value};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::client::{Client, TaskMessageData};
use crate::config::Config;
use crate::execenv::context::{
    cleanup_sidecars, TaskContextMarkerFile, TASK_CONTEXT_MARKER_MANAGED_BY,
    TASK_CONTEXT_MARKER_REL_PATH,
};
use crate::execenv::codex_home::codex_resume_rollout_present;
use crate::execenv::execenv::{
    ensure_task_temp_dir, predict_root_dir, prepare, read_managed_env_provenance, reuse,
    Environment, MANAGED_ENV_PROVENANCE_MANAGED_BY,
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
use crate::plugin_hook_mcp::{start_task_plugin_hook_mcp, PluginHookInvoker};
use crate::poisoned::{
    classify_poisoned_error, classify_poisoned_output, classify_resume_unsafe_timeout,
    classify_resume_unsafe_transport,
};
use crate::production_services::{ProviderRuntimeAdapter, ProviderRuntimeContext};
use crate::prompt::build_prompt;
use crate::provider_registration::{RuntimeLaunchRegistry, RuntimeLaunchSpec};
use crate::remote_mcp_broker::{
    merge_task_remote_mcp_config, start_task_remote_mcp_brokers,
    RemoteMCPCredentialResolver,
};
use crate::repocache::Ctx;
use crate::runtime_registry::RuntimeRegistry;
use crate::skill_cache::{
    build_manifest, validate_skill_bundle, SkillBundleCache, SkillBundleFile, SkillBundleSkill,
    SOURCE_PLUGIN,
};
use crate::task_execution::{TaskRunFailure, TaskRunOutcome};
use crate::types::{
    RuntimeExecutionTarget, SkillData, SkillFileRefData, SkillRefData, Task, TaskResult,
    TaskUsageEntry,
};

const TRANSCRIPT_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const TRANSCRIPT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const TRANSCRIPT_DRAIN_GRACE: Duration = Duration::from_secs(10);
const CODEX_ROLLOUT_FLUSH_WAIT: Duration = Duration::from_secs(2);
const CODEX_ROLLOUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TRANSCRIPT_BATCH_LIMIT: usize = 32;
const TOOL_OUTPUT_BYTES: usize = 8 * 1024;
const TOOL_INPUT_BYTES: usize = 64 * 1024;
const PREPARE_LEASE_REFRESH: Duration = Duration::from_secs(15);
const PREPARE_LEASE_TIMEOUT: Duration = Duration::from_secs(10);
const PREPARE_LEASE_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const SKILL_BUNDLE_RESOLVE_MIN_TIMEOUT: Duration = Duration::from_secs(30);
const SKILL_BUNDLE_RESOLVE_MAX_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SKILL_BUNDLE_RESOLVE_MIN_THROUGHPUT: i64 = 50 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TaskModelSelection {
    model: String,
    thinking_level: String,
    service_tier: String,
}

/// Real provider adapter for protocol families implemented by `cordy-agent`.
/// Metadata-only runtimes fail at `build_backend`; no provider can turn into a
/// pretend success path.
pub struct ProductionProviderAdapter {
    config: Arc<Config>,
    model_cache: Arc<CatalogCache>,
    skill_cache: Arc<SkillBundleCache>,
    local_paths: Arc<LocalPathLocker>,
    started_at: Instant,
    active_tasks: AtomicI64,
    running_tasks: AtomicI64,
    resource_wait_tasks: AtomicI64,
}

impl ProductionProviderAdapter {
    pub fn new(config: Arc<Config>) -> Self {
        let skill_cache_root = Path::new(&config.workspaces_root)
            .join(".skill-cache")
            .join("v1")
            .to_string_lossy()
            .into_owned();
        Self {
            config,
            model_cache: Arc::new(CatalogCache::default()),
            skill_cache: Arc::new(SkillBundleCache::new(&skill_cache_root)),
            local_paths: Arc::new(LocalPathLocker::new()),
            started_at: Instant::now(),
            active_tasks: AtomicI64::new(0),
            running_tasks: AtomicI64::new(0),
            resource_wait_tasks: AtomicI64::new(0),
        }
    }

    /// Resolves the values a task will actually pass to the provider. The
    /// catalog is deliberately loaded at most once: qualification and both
    /// capability checks share one discovery result, while providers that do
    /// not need a catalog avoid spawning a CLI subprocess entirely.
    async fn resolve_task_model_selection(
        &self,
        ctx: &Ctx,
        task_id: &str,
        target: &RuntimeExecutionTarget,
        launch: &RuntimeLaunchSpec,
        mut selection: TaskModelSelection,
    ) -> TaskModelSelection {
        let capability_checks_pending =
            !selection.thinking_level.is_empty() || !selection.service_tier.is_empty();
        let needs_catalog = (!selection.model.is_empty()
            && (cordy_agent::registry::model_selector_must_be_provider_qualified(
                &target.provider,
            ) || capability_checks_pending))
            || (!selection.thinking_level.is_empty()
                && !(target.provider == "codex" && selection.model.is_empty()));

        let catalog = if needs_catalog {
            Some(
                cordy_agent::registry::discover_models(
                    &target.provider,
                    cordy_agent::BackendConfig {
                        command: cordy_agent::RuntimeCommand::new(
                            launch.command_path.clone(),
                            launch.fixed_args.clone(),
                        ),
                        env: BTreeMap::new(),
                        builtin_runtime: target.profile_id.is_empty(),
                    },
                    &self.model_cache,
                    ctx.token().clone(),
                    Duration::ZERO,
                )
                .await,
            )
        } else {
            None
        };

        if !selection.model.is_empty()
            && (cordy_agent::registry::model_selector_must_be_provider_qualified(&target.provider)
                || capability_checks_pending)
        {
            match catalog.as_ref() {
                Some(Ok(catalog)) => {
                    let (qualified, rewritten) =
                        cordy_agent::model::qualify_model_id(catalog, &selection.model);
                    if rewritten {
                        tracing::info!(
                            task = %task_id,
                            provider = %target.provider,
                            configured_model = %selection.model,
                            model = %qualified,
                            "model qualified against the runtime catalog"
                        );
                        selection.model = qualified;
                    }
                }
                Some(Err(error)) => {
                    tracing::warn!(
                        task = %task_id,
                        provider = %target.provider,
                        model = %selection.model,
                        %error,
                        "model catalog lookup failed; using the configured model as-is"
                    );
                }
                None => {}
            }
        }

        if !selection.service_tier.is_empty() {
            let valid = if target.provider != "codex" || selection.model.is_empty() {
                false
            } else {
                match catalog.as_ref() {
                    Some(Ok(catalog)) => cordy_agent::model::validate_service_tier(
                        catalog,
                        &target.provider,
                        &selection.model,
                        &selection.service_tier,
                    ),
                    Some(Err(error)) => {
                        tracing::warn!(
                            task = %task_id,
                            provider = %target.provider,
                            model = %selection.model,
                            service_tier = %selection.service_tier,
                            %error,
                            "service_tier catalog lookup failed; passing through"
                        );
                        true
                    }
                    None => false,
                }
            };
            if !valid {
                tracing::warn!(
                    task = %task_id,
                    provider = %target.provider,
                    model = %selection.model,
                    service_tier = %selection.service_tier,
                    "service_tier is not valid for this provider/model; skipping injection"
                );
                selection.service_tier.clear();
            }
        }

        if !selection.thinking_level.is_empty() {
            let valid = if target.provider == "codex" && selection.model.is_empty() {
                false
            } else {
                match catalog.as_ref() {
                    Some(Ok(catalog)) => cordy_agent::model::validate_thinking_level(
                        catalog,
                        &target.provider,
                        &selection.model,
                        &selection.thinking_level,
                    ),
                    Some(Err(error)) => {
                        tracing::warn!(
                            task = %task_id,
                            provider = %target.provider,
                            model = %selection.model,
                            thinking_level = %selection.thinking_level,
                            %error,
                            "thinking_level catalog lookup failed; passing through"
                        );
                        true
                    }
                    None => false,
                }
            };
            if !valid {
                tracing::warn!(
                    task = %task_id,
                    provider = %target.provider,
                    model = %selection.model,
                    thinking_level = %selection.thinking_level,
                    "thinking_level is not valid for this provider/model; skipping injection"
                );
                selection.thinking_level.clear();
            }
        }

        selection
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
        // Planning needs a non-empty placeholder before execenv claims the
        // root. It is replaced after preparation by the short private dir.
        let planned_temp_dir = Path::new(&predicted_root)
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
            temp_dir: planned_temp_dir,
            default_model,
            codex_version: launch.version.clone(),
            openclaw_bin: (target.provider == "openclaw")
                .then(|| launch.command_path.clone())
                .unwrap_or_default(),
            path: provider_path(),
            ..ProviderExecutionInputs::default()
        };
        let client = runtime.client();
        if let Some(agent) = task.agent.as_ref() {
            inputs.cursor_mcp_auth_source = agent
                .custom_env
                .as_ref()
                .and_then(|env| env.get("CURSOR_MCP_AUTH_SOURCE"))
                .cloned()
                .unwrap_or_default();
            if let Some(agent_mcp_config) = agent.mcp_config.as_ref() {
                match crate::runtime_mcp::merge_runtime_and_agent_mcp_config(
                    &target.provider,
                    agent_mcp_config,
                ) {
                    Ok(effective) => inputs.effective_mcp_config = effective,
                    Err(error) => tracing::warn!(
                        task = %task.id,
                        provider = %target.provider,
                        %error,
                        "mcp_config: runtime merge failed; using agent configuration only"
                    ),
                }
            }
        }
        let credential_resolver: RemoteMCPCredentialResolver = {
            let client = Arc::clone(&client);
            let daemon_token = task.remote_mcp_daemon_token.clone();
            let task_id = task.id.clone();
            Arc::new(move |resolve_ctx, contribution_id| {
                let client = Arc::clone(&client);
                let daemon_token = daemon_token.clone();
                let task_id = task_id.clone();
                Box::pin(async move {
                    client
                        .resolve_remote_mcp_credential(
                            &resolve_ctx,
                            &daemon_token,
                            &task_id,
                            &contribution_id,
                        )
                        .await
                })
            })
        };
        let remote_mcp = match start_task_remote_mcp_brokers(
            &ctx,
            &ctx,
            &task.id,
            &target.provider,
            &task.remote_mcp_connections,
            Some(credential_resolver),
        )
        .await
        {
            Ok(startup) => startup,
            Err(error) => {
                return failed(
                    error.context("prepare Remote MCP broker"),
                    None,
                )
            }
        };
        for diagnostic in &remote_mcp.diagnostics {
            tracing::warn!(task = %task.id, reason = %diagnostic, "Remote MCP degraded");
        }
        if let Some(error) = remote_mcp.error {
            return failed(error.context("prepare Remote MCP broker"), None);
        }
        if let Some(overlay) = remote_mcp.config {
            let base = inputs
                .effective_mcp_config
                .as_ref()
                .or_else(|| task.agent.as_ref().and_then(|agent| agent.mcp_config.as_ref()))
                .map(Value::to_string)
                .unwrap_or_default();
            let merged = match merge_task_remote_mcp_config(&base, &overlay.to_string())
                .and_then(|raw| serde_json::from_str(&raw).map_err(anyhow::Error::new))
            {
                Ok(merged) => merged,
                Err(error) => {
                    return failed(
                        error.context("merge Remote MCP broker configuration"),
                        None,
                    )
                }
            };
            inputs.effective_mcp_config = Some(merged);
        }
        // Own every loopback broker until provider execution and environment
        // finalization finish. Drop closes all listeners on every return path.
        let _remote_mcp_brokers = remote_mcp.set;
        let plugin_invoke: PluginHookInvoker = {
            let client = Arc::clone(&client);
            let daemon_token = task.remote_mcp_daemon_token.clone();
            Arc::new(
                move |call_ctx, task_id, installation_id, hook_key, input| {
                    let client = Arc::clone(&client);
                    let daemon_token = daemon_token.clone();
                    let task_id = task_id.to_string();
                    let installation_id = installation_id.to_string();
                    let hook_key = hook_key.to_string();
                    let input = input.clone();
                    Box::pin(async move {
                        client
                            .invoke_agent_plugin_hook(
                                call_ctx,
                                &daemon_token,
                                &task_id,
                                &installation_id,
                                &hook_key,
                                Some(input),
                            )
                            .await
                            .map(|output| output.unwrap_or(Value::Null))
                    })
                },
            )
        };
        let (plugin_overlay, plugin_hook_mcp) =
            match start_task_plugin_hook_mcp(&ctx, &task.id, &task.plugin_hook_tools, plugin_invoke)
                .await
            {
                Ok(started) => started,
                Err(error) => {
                    tracing::warn!(
                        task = %task.id,
                        %error,
                        "plugin hook tools unavailable"
                    );
                    (None, None)
                }
            };
        if let Some(overlay) = plugin_overlay {
            let base = inputs
                .effective_mcp_config
                .as_ref()
                .or_else(|| task.agent.as_ref().and_then(|agent| agent.mcp_config.as_ref()))
                .map(Value::to_string)
                .unwrap_or_default();
            match merge_task_remote_mcp_config(&base, &overlay.to_string())
                .and_then(|raw| serde_json::from_str(&raw).map_err(anyhow::Error::new))
            {
                Ok(merged) => inputs.effective_mcp_config = Some(merged),
                Err(error) => tracing::warn!(
                    task = %task.id,
                    %error,
                    "could not merge plugin hook MCP config"
                ),
            }
        }
        // Keep the local tool server alive through provider finalization;
        // Drop closes it on every early return as well.
        let _plugin_hook_mcp = plugin_hook_mcp;
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
        let prepare_lease = PrepareLeaseExtender::start(
            ctx.clone(),
            Arc::clone(&client),
            task.runtime_id.clone(),
            task.id.clone(),
        );
        if let Err(error) = self
            .ensure_task_skill_bundles(&ctx, &client, &mut task)
            .await
        {
            return failed_with_reason(
                error,
                cordy_task_failure::Reason::SKILL_BUNDLE_UNAVAILABLE.as_str(),
                None,
            );
        }
        let explicit_model = task
            .agent
            .as_ref()
            .is_some_and(|agent| !agent.model.is_empty());
        let selection = self
            .resolve_task_model_selection(
                &ctx,
                &task.id,
                &target,
                &launch,
                TaskModelSelection {
                    model: task
                        .agent
                        .as_ref()
                        .map(|agent| agent.model.clone())
                        .filter(|model| !model.is_empty())
                        .unwrap_or_else(|| inputs.default_model.clone()),
                    thinking_level: task
                        .agent
                        .as_ref()
                        .map(|agent| agent.thinking_level.clone())
                        .unwrap_or_default(),
                    service_tier: task
                        .agent
                        .as_ref()
                        .map(|agent| agent.service_tier.clone())
                        .unwrap_or_default(),
                },
            )
            .await;
        if explicit_model {
            if let Some(agent) = task.agent.as_mut() {
                agent.model = selection.model;
            }
        } else {
            inputs.default_model = selection.model;
        }
        if let Some(agent) = task.agent.as_mut() {
            agent.thinking_level = selection.thinking_level;
            agent.service_tier = selection.service_tier;
        }
        let mut plan = match ProviderExecutionPlan::build(&self.config, &task, &target, inputs) {
            Ok(plan) => plan,
            Err(error) => return failed(error, None),
        };
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
        let task_temp_dir = match ensure_task_temp_dir(
            &environment.root_dir,
            &task.workspace_id,
            &task.id,
        ) {
            Ok(directory) => directory,
            Err(error) => {
                let outcome = failed(error.context("prepare task temp dir"), Some(&environment));
                return finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
            }
        };
        let Some(task_temp_dir_path) = task_temp_dir.path().to_str() else {
            let outcome = failed(
                anyhow::anyhow!("task temp directory path is not valid UTF-8"),
                Some(&environment),
            );
            close_task_temp_dir(&task.id, task_temp_dir);
            return finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
        };
        if let Err(error) = plan.set_task_temp_dir(task_temp_dir_path) {
            let outcome = failed(
                error.context("bind task temp directory"),
                Some(&environment),
            );
            close_task_temp_dir(&task.id, task_temp_dir);
            return finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
        }
        let run = async {
            client
                .start_task(&ctx, &task.id)
                .await
                .map_err(|error| anyhow::anyhow!("start task failed: {error}"))?;
            // The dispatched-task lease remains owned through temp allocation
            // and stops only after the server confirms the running transition.
            prepare_lease.stop().await;
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
            let requested_session_id = plan.resume_session_id().to_string();
            let bound = plan.bind_environment(
                &environment,
                PreparedEnvironmentInputs {
                    cancellation: ctx.token().clone(),
                    openclaw_include_roots: environment.openclaw_include_root.clone(),
                    ..PreparedEnvironmentInputs::default()
                },
            )?;
            let backend_config = runtime.backend_config(
                &task.workspace_id,
                &target,
                bound.child_env.into_inner(),
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
            let _running = CounterGuard::new(&self.running_tasks);
            let mut transcript = TranscriptBatch::default();
            let (first, first_tools) = execute_and_drain(
                backend.as_ref(),
                &ctx,
                &client,
                &task.id,
                &environment.work_dir,
                &environment.codex_home,
                &prompt,
                bound.options,
                &mut transcript,
            )
            .await
            .map_err(|error| anyhow::anyhow!("execute {}: {error}", target.provider))?;

            let (mut result, retired_session_id) = if !should_retry_with_fresh_session(
                &first,
                &requested_session_id,
                first_tools,
                &target.provider,
            ) {
                (first, String::new())
            } else {
                tracing::warn!(
                    task = %task.id,
                    provider = %target.provider,
                    "session resume failed; retrying once with a fresh session"
                );
                task.prior_session_id.clear();
                task.prior_session_resume_unavailable = true;
                plan.drop_resume();
                let fresh = plan.bind_environment(
                    &environment,
                    PreparedEnvironmentInputs {
                        cancellation: ctx.token().clone(),
                        openclaw_include_roots: environment.openclaw_include_root.clone(),
                        ..PreparedEnvironmentInputs::default()
                    },
                )?;
                transcript.begin_attempt();
                let retry = execute_and_drain(
                    backend.as_ref(),
                    &ctx,
                    &client,
                    &task.id,
                    &environment.work_dir,
                    &environment.codex_home,
                    &build_prompt(task.clone(), &target.provider),
                    fresh.options,
                    &mut transcript,
                )
                .await;
                (
                    reconcile_fresh_retry_result(first, retry),
                    requested_session_id.clone(),
                )
            };
            let session_rollout_missing = withhold_missing_codex_rollout(
                &mut result,
                &environment.codex_home,
                CODEX_ROLLOUT_FLUSH_WAIT,
            )
            .await;
            Ok((
                result,
                requested_session_id,
                retired_session_id,
                session_rollout_missing,
            ))
        }
        .await;

        let mut outcome = match run {
            Ok((result, requested_session_id, retired_session_id, session_rollout_missing)) => {
                let mut outcome = result_outcome(
                    &target.provider,
                    result,
                    &environment,
                    &requested_session_id,
                    &retired_session_id,
                );
                outcome.result.session_rollout_missing = session_rollout_missing;
                outcome
            }
            Err(error) => failed(error, Some(&environment)),
        };
        close_task_temp_dir(&task.id, task_temp_dir);
        outcome = finalize_environment(outcome, &mut environment, assignment.as_ref()).await;
        drop(path_guard);
        outcome
    }

    async fn ensure_task_skill_bundles(
        &self,
        ctx: &Ctx,
        client: &Client,
        task: &mut Task,
    ) -> anyhow::Result<()> {
        let Some(agent) = task.agent.as_ref() else {
            return Ok(());
        };
        if agent.skill_refs.is_empty() {
            return Ok(());
        }

        // Copy the refs before mutating agent.skills after all cache/network
        // work completes. This also keeps the borrow independent of each
        // awaited download.
        let refs = agent.skill_refs.clone();
        let mut resolved = HashMap::with_capacity(refs.len());
        let mut misses = Vec::new();
        for skill_ref in &refs {
            let cached = self
                .skill_cache
                .with_ref_lock(&task.workspace_id, skill_ref, || {
                    self.skill_cache.load(&task.workspace_id, skill_ref)
                });
            if let Some(bundle) = cached {
                resolved.insert(skill_ref_key(&skill_ref.source, &skill_ref.id), bundle);
            } else {
                misses.push(skill_ref.clone());
            }
        }

        for skill_ref in misses {
            let started = Instant::now();
            let bundle = self
                .resolve_skill_bundle(ctx, client, task, &skill_ref)
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "skill bundle unavailable: skill {:?} (id={}, {} bytes) after {:?}: {error}",
                        skill_ref.name,
                        skill_ref.id,
                        skill_ref.size_bytes,
                        started.elapsed(),
                    )
                })?;
            resolved.insert(skill_ref_key(&bundle.source, &bundle.id), bundle);
        }

        let skills = refs
            .iter()
            .map(|skill_ref| {
                resolved
                    .get(&skill_ref_key(&skill_ref.source, &skill_ref.id))
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "skill bundle missing after resolve: skill_id={} source={} hash={}",
                            skill_ref.id,
                            skill_ref.source,
                            skill_ref.hash,
                        )
                    })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        task.agent
            .as_mut()
            .expect("agent remains present while resolving skill bundles")
            .skills = skills;
        Ok(())
    }

    async fn resolve_skill_bundle(
        &self,
        ctx: &Ctx,
        client: &Client,
        task: &Task,
        skill_ref: &SkillRefData,
    ) -> anyhow::Result<SkillData> {
        let timeout = skill_bundle_resolve_timeout(skill_ref.size_bytes);
        let bundle = tokio::time::timeout(
            timeout,
            client.resolve_skill_bundle(ctx, &task.runtime_id, &task.id, skill_ref.clone()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("resolve skill bundle timed out after {timeout:?}"))??;

        if bundle.source != skill_ref.source || bundle.id != skill_ref.id {
            anyhow::bail!(
                "resolve skill bundle returned wrong skill: requested source={} id={}, got source={} id={}",
                skill_ref.source,
                skill_ref.id,
                bundle.source,
                bundle.id,
            );
        }

        let derived_ref = skill_ref_from_bundle(&bundle);
        let validation_ref = if skill_ref.source == SOURCE_PLUGIN {
            skill_ref.clone()
        } else {
            derived_ref
        };
        if !validate_skill_bundle(&validation_ref, &bundle) {
            anyhow::bail!(
                "resolve skill bundle returned invalid bundle: skill_id={} source={} hash={}",
                bundle.id,
                bundle.source,
                bundle.hash,
            );
        }

        let store_result =
            self.skill_cache
                .with_ref_lock(&task.workspace_id, &validation_ref, || {
                    self.skill_cache.store(&task.workspace_id, &bundle)
                });
        if let Err(error) = store_result {
            tracing::warn!(
                workspace_id = %task.workspace_id,
                skill_id = %bundle.id,
                source = %bundle.source,
                hash = %bundle.hash,
                %error,
                "skill bundle cache store failed; continuing with downloaded bundle"
            );
        }
        Ok(bundle)
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
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
        client: Arc<Client>,
        launch_registry: Arc<RuntimeLaunchRegistry>,
    ) {
        let Some(workspace_id) = registry.workspace_for_runtime(&runtime_id) else {
            tracing::debug!(%runtime_id, "dropping heartbeat actions for unknown runtime");
            return;
        };
        let Some(target) = registry.execution_target_for_runtime(&runtime_id) else {
            tracing::debug!(%runtime_id, "dropping heartbeat actions without runtime identity");
            return;
        };

        if let Some(pending) = ack.pending_model_list {
            tokio::spawn(handle_model_list(
                ctx.child(),
                Arc::clone(&client),
                Arc::clone(&launch_registry),
                Arc::clone(&self.model_cache),
                workspace_id.clone(),
                runtime_id.clone(),
                target.clone(),
                pending.id,
            ));
        }
        if let Some(pending) = ack.pending_local_skills {
            tokio::spawn(handle_local_skill_list(
                ctx.child(),
                Arc::clone(&client),
                runtime_id.clone(),
                target.provider.clone(),
                pending.id,
            ));
        }

        // Prefer the batch field, falling back to the singular field for old
        // servers, matching daemon.go's heartbeat compatibility contract.
        if ack.pending_local_skill_imports.is_empty() {
            if let Some(pending) = ack.pending_local_skill_import {
                tokio::spawn(handle_local_skill_import(
                    ctx.child(),
                    Arc::clone(&client),
                    runtime_id,
                    target.provider,
                    pending.id,
                    pending.skill_key,
                ));
            }
        } else {
            for pending in ack.pending_local_skill_imports {
                tokio::spawn(handle_local_skill_import(
                    ctx.child(),
                    Arc::clone(&client),
                    runtime_id.clone(),
                    target.provider.clone(),
                    pending.id,
                    pending.skill_key,
                ));
            }
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

async fn handle_model_list(
    ctx: Ctx,
    client: Arc<Client>,
    launch_registry: Arc<RuntimeLaunchRegistry>,
    cache: Arc<CatalogCache>,
    workspace_id: String,
    runtime_id: String,
    target: RuntimeExecutionTarget,
    request_id: String,
) {
    tracing::info!(
        %runtime_id,
        %request_id,
        provider = %target.provider,
        "runtime model list requested"
    );

    let payload = match launch_registry.resolve(&workspace_id, &target) {
        None => json!({
            "status": "failed",
            "error": format!("no accepted launch registered for runtime {runtime_id}"),
        }),
        Some(launch) if launch.command_path.trim().is_empty() => json!({
            "status": "failed",
            "error": format!("accepted launch for provider {} has no executable path", target.provider),
        }),
        Some(launch) => {
            let config = cordy_agent::BackendConfig {
                command: cordy_agent::RuntimeCommand::new(launch.command_path, launch.fixed_args),
                env: BTreeMap::new(),
                builtin_runtime: target.profile_id.is_empty(),
            };
            match cordy_agent::registry::discover_models(
                &target.provider,
                config,
                &cache,
                ctx.token().clone(),
                Duration::ZERO,
            )
            .await
            {
                Ok(catalog) => json!({
                    "status": "completed",
                    "models": catalog.models,
                    "supported": cordy_agent::registry::model_selection_supported(&target.provider),
                    "fallback": catalog.fallback,
                }),
                Err(error) => json!({
                    "status": "failed",
                    "error": error.to_string(),
                }),
            }
        }
    };

    if let Err(error) = client
        .report_model_list_result(&ctx, &runtime_id, &request_id, payload)
        .await
    {
        tracing::warn!(%runtime_id, %request_id, %error, "report runtime model list failed");
    }
}

async fn handle_local_skill_list(
    ctx: Ctx,
    client: Arc<Client>,
    runtime_id: String,
    provider: String,
    request_id: String,
) {
    tracing::info!(
        %runtime_id,
        %request_id,
        %provider,
        "runtime local skills requested"
    );

    let payload = match crate::local_skills::list_runtime_local_skills(&provider) {
        Err(error) => json!({
            "status": "failed",
            "error": error.to_string(),
        }),
        Ok((skills, supported)) => {
            let (mcp_servers, mcp_supported) =
                match crate::runtime_mcp::list_runtime_local_mcp_servers(&provider) {
                    Ok((servers, supported)) => (servers, supported),
                    Err(error) => {
                        tracing::warn!(
                            %runtime_id,
                            %provider,
                            %error,
                            "runtime local MCP discovery failed"
                        );
                        (Vec::new(), false)
                    }
                };
            json!({
                "status": "completed",
                "skills": skills,
                "supported": supported,
                "mcp_servers": mcp_servers,
                "mcp_supported": mcp_supported,
            })
        }
    };

    if let Err(error) = client
        .report_local_skill_list_result(&ctx, &runtime_id, &request_id, payload)
        .await
    {
        tracing::warn!(%runtime_id, %request_id, %error, "report runtime local skills failed");
    }
}

async fn handle_local_skill_import(
    ctx: Ctx,
    client: Arc<Client>,
    runtime_id: String,
    provider: String,
    request_id: String,
    skill_key: String,
) {
    tracing::info!(
        %runtime_id,
        %request_id,
        %provider,
        %skill_key,
        "runtime local skill import requested"
    );

    let payload = match crate::local_skills::load_runtime_local_skill_bundle(&provider, &skill_key)
    {
        Err(error) => json!({
            "status": "failed",
            "error": error.to_string(),
        }),
        Ok((_, false)) => json!({
            "status": "failed",
            "error": format!("provider {provider:?} does not expose runtime local skills"),
        }),
        Ok((skill, true)) => json!({
            "status": "completed",
            "skill": skill,
        }),
    };

    if let Err(error) = client
        .report_local_skill_import_result(&ctx, &runtime_id, &request_id, payload)
        .await
    {
        tracing::warn!(%runtime_id, %request_id, %error, "report runtime local skill import failed");
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

async fn codex_session_resumable(codex_home: &str, session_id: &str, wait: Duration) -> bool {
    if codex_home.is_empty() || session_id.is_empty() {
        return true;
    }
    if codex_resume_rollout_present(codex_home, session_id) {
        return true;
    }
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return codex_resume_rollout_present(codex_home, session_id);
        }
        tokio::time::sleep(CODEX_ROLLOUT_POLL_INTERVAL.min(deadline - now)).await;
        if codex_resume_rollout_present(codex_home, session_id) {
            return true;
        }
    }
}

async fn wait_codex_rollout_present(
    owner: &CancellationToken,
    codex_home: &str,
    session_id: &str,
) -> bool {
    if codex_home.is_empty() || session_id.is_empty() {
        return true;
    }
    if codex_resume_rollout_present(codex_home, session_id) {
        return true;
    }
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + CODEX_ROLLOUT_POLL_INTERVAL,
        CODEX_ROLLOUT_POLL_INTERVAL,
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = owner.cancelled() => {
                return codex_resume_rollout_present(codex_home, session_id);
            }
            _ = ticker.tick() => {
                if codex_resume_rollout_present(codex_home, session_id) {
                    return true;
                }
            }
        }
    }
}

async fn withhold_missing_codex_rollout(
    result: &mut ExecutionResult,
    codex_home: &str,
    wait: Duration,
) -> bool {
    if codex_session_resumable(codex_home, &result.session_id, wait).await {
        return false;
    }
    tracing::warn!(
        session_id = %result.session_id,
        %codex_home,
        status = %result.status,
        "Codex session rollout is missing; withholding the resume pointer"
    );
    result.session_id.clear();
    true
}

async fn drain_session(
    ctx: &Ctx,
    client: &Arc<Client>,
    task_id: &str,
    work_dir: &str,
    codex_home: &str,
    session: Session,
    transcript: &mut TranscriptBatch,
) -> anyhow::Result<ExecutionResult> {
    let Session {
        mut messages,
        mut result,
    } = session;
    let mut ticker = tokio::time::interval(TRANSCRIPT_FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut terminal: Option<ExecutionResult> = None;
    let mut cancelled = false;
    let mut messages_closed = false;
    let mut result_closed = false;
    let mut drain_deadline = Box::pin(tokio::time::sleep(Duration::from_secs(365 * 24 * 3600)));
    let mut drain_armed = false;
    let pin_owner = CancellationToken::new();
    let mut pin_waiters = JoinSet::new();

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
                        if let Some(session_id) = transcript.push(message) {
                            let client = Arc::clone(client);
                            let task_id = task_id.to_string();
                            let work_dir = work_dir.to_string();
                            let codex_home = codex_home.to_string();
                            let pin_owner = pin_owner.clone();
                            pin_waiters.spawn(async move {
                                if !wait_codex_rollout_present(
                                    &pin_owner,
                                    &codex_home,
                                    &session_id,
                                )
                                .await
                                {
                                    tracing::debug!(
                                        task = %task_id,
                                        %session_id,
                                        "skip pinning Codex session without a rollout"
                                    );
                                    return;
                                }
                                let pin_ctx = Ctx::new();
                                let pin = client.pin_task_session(
                                    &pin_ctx,
                                    &task_id,
                                    &session_id,
                                    &work_dir,
                                );
                                match tokio::time::timeout(TRANSCRIPT_REQUEST_TIMEOUT, pin).await {
                                    Ok(Ok(())) => {}
                                    Ok(Err(error)) => tracing::debug!(task = %task_id, %error, "pin task session failed"),
                                    Err(_) => tracing::debug!(task = %task_id, "pin task session timed out"),
                                }
                            });
                        }
                        if transcript.ready() {
                            flush_transcript(client, task_id, transcript).await;
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
            _ = ticker.tick() => flush_transcript(client, task_id, transcript).await,
            () = &mut drain_deadline, if drain_armed => {
                tracing::warn!(task = %task_id, "provider transcript did not close within drain grace");
                break;
            }
        }
    }
    flush_transcript(client, task_id, transcript).await;
    pin_owner.cancel();
    while pin_waiters.join_next().await.is_some() {
    }
    Ok(terminal.unwrap_or_else(|| ExecutionResult {
        status: "failed".to_string(),
        error: "provider messages closed without a terminal result".to_string(),
        ..ExecutionResult::default()
    }))
}

#[derive(Default)]
struct TranscriptBatch {
    next_seq: i32,
    messages: Vec<TaskMessageData>,
    tools: HashMap<String, String>,
    tool_use_count: usize,
    session_pinned: bool,
}

impl TranscriptBatch {
    fn begin_attempt(&mut self) {
        self.tools.clear();
        self.session_pinned = false;
    }

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
            self.tool_use_count = self.tool_use_count.saturating_add(1);
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

async fn execute_and_drain(
    backend: &dyn Backend,
    ctx: &Ctx,
    client: &Arc<Client>,
    task_id: &str,
    work_dir: &str,
    codex_home: &str,
    prompt: &str,
    options: ExecOptions,
    transcript: &mut TranscriptBatch,
) -> anyhow::Result<(ExecutionResult, usize)> {
    let tools_before = transcript.tool_use_count;
    let session = backend.execute(prompt, options).await?;
    let result = drain_session(
        ctx,
        client,
        task_id,
        work_dir,
        codex_home,
        session,
        transcript,
    )
    .await?;
    Ok((
        result,
        transcript.tool_use_count.saturating_sub(tools_before),
    ))
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
    tool_uses: usize,
    provider: &str,
) -> bool {
    if result.status != "failed" || requested_session_id.is_empty() || tool_uses != 0 {
        return false;
    }
    if result.resume_rejected
        || cordy_task_failure::unresumable_history(&result.error)
        || cordy_task_failure::auth_method_unresolved(&result.error)
    {
        return true;
    }
    cordy_agent::registry::resume_rejection_undetectable(provider)
        && result.session_id.is_empty()
        && fresh_session_may_help(&result.error)
}

fn fresh_session_may_help(error: &str) -> bool {
    let reason = cordy_task_failure::classify(error);
    ![
        cordy_task_failure::Reason::AGENT_PROVIDER_NETWORK,
        cordy_task_failure::Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT,
        cordy_task_failure::Reason::AGENT_PROVIDER_QUOTA_LIMIT,
        cordy_task_failure::Reason::AGENT_PROVIDER_SERVER_ERROR,
        cordy_task_failure::Reason::AGENT_PROVIDER_AUTH_OR_ACCESS,
        cordy_task_failure::Reason::AGENT_MISSING_CONFIG,
        cordy_task_failure::Reason::AGENT_MODEL_NOT_FOUND_OR_UNAVAILABLE,
        cordy_task_failure::Reason::AGENT_RUNTIME_MISSING_EXECUTABLE,
        cordy_task_failure::Reason::AGENT_RUNTIME_VERSION_UNSUPPORTED,
        cordy_task_failure::Reason::AGENT_TIMEOUT,
    ]
    .contains(&reason)
}

fn merge_usage(
    mut first: BTreeMap<String, TokenUsage>,
    retry: BTreeMap<String, TokenUsage>,
) -> BTreeMap<String, TokenUsage> {
    for (model, next) in retry {
        let current = first.entry(model).or_default();
        current.input_tokens = current.input_tokens.saturating_add(next.input_tokens);
        current.output_tokens = current.output_tokens.saturating_add(next.output_tokens);
        current.cache_read_tokens = current
            .cache_read_tokens
            .saturating_add(next.cache_read_tokens);
        current.cache_write_tokens = current
            .cache_write_tokens
            .saturating_add(next.cache_write_tokens);
        current.cost_usd_ticks = current.cost_usd_ticks.saturating_add(next.cost_usd_ticks);
    }
    first
}

fn reconcile_fresh_retry_result(
    mut first: ExecutionResult,
    retry: anyhow::Result<(ExecutionResult, usize)>,
) -> ExecutionResult {
    let Ok((mut retry, _retry_tools)) = retry else {
        return first;
    };
    let usage = merge_usage(std::mem::take(&mut first.usage), std::mem::take(&mut retry.usage));
    if !retry.session_id.is_empty() || retry.status == "completed" {
        retry.usage = usage;
        retry
    } else {
        first.usage = usage;
        first
    }
}

fn result_outcome(
    provider: &str,
    result: ExecutionResult,
    env: &Environment,
    requested_session_id: &str,
    retired_session_id: &str,
) -> TaskRunOutcome {
    let mut retired_session_id = retired_session_id.to_string();
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
    let (status, comment, failure_reason) = match result.status.as_str() {
        "completed" => match classify_poisoned_output(&result.output) {
            Some(reason) => ("blocked", result.output, reason.to_string()),
            None => ("completed", result.output, String::new()),
        },
        "cancelled" => (
            "cancelled",
            if result.error.is_empty() {
                "task cancelled by server".to_string()
            } else {
                result.error
            },
            "cancelled".to_string(),
        ),
        "timeout" => {
            let comment = if result.error.is_empty() {
                format!("{provider} timed out")
            } else {
                result.error
            };
            let reason = classify_resume_unsafe_timeout(provider, &comment).unwrap_or("timeout");
            ("blocked", comment, reason.to_string())
        }
        "idle_watchdog" => ("blocked", result.error, "idle_watchdog".to_string()),
        _ => {
            let comment = if result.error.is_empty() {
                format!("{provider} execution {}", result.status)
            } else {
                result.error
            };
            let failure_reason = if let Some(reason) = classify_poisoned_error(&comment) {
                reason.to_string()
            } else if let Some(reason) = classify_resume_unsafe_transport(provider, &comment) {
                if retired_session_id.is_empty() {
                    retired_session_id = requested_session_id.to_string();
                }
                reason.to_string()
            } else if result.resume_rejected && !requested_session_id.is_empty() {
                if retired_session_id.is_empty() {
                    retired_session_id = requested_session_id.to_string();
                }
                "resume_rejected".to_string()
            } else {
                cordy_task_failure::classify(&comment).to_string()
            };
            ("blocked", comment, failure_reason)
        }
    };
    TaskRunOutcome {
        result: TaskResult {
            status: status.to_string(),
            comment,
            session_id: result.session_id,
            work_dir: env.work_dir.clone(),
            env_root: env.root_dir.clone(),
            failure_reason,
            retired_session_id,
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

fn close_task_temp_dir(task_id: &str, directory: tempfile::TempDir) {
    let path = directory.path().to_path_buf();
    if let Err(error) = directory.close() {
        tracing::warn!(task = %task_id, path = %path.display(), %error, "task temp directory cleanup failed");
    }
}

fn failed_with_reason(
    error: anyhow::Error,
    failure_reason: &str,
    environment: Option<&Environment>,
) -> TaskRunOutcome {
    let mut outcome = failed(error, environment);
    if let Some(failure) = outcome.failure.as_mut() {
        failure.failure_reason = failure_reason.to_string();
    }
    outcome
}

fn skill_bundle_resolve_timeout(size_bytes: i64) -> Duration {
    if size_bytes <= 0 {
        return SKILL_BUNDLE_RESOLVE_MIN_TIMEOUT;
    }
    let scaled_seconds = (size_bytes / SKILL_BUNDLE_RESOLVE_MIN_THROUGHPUT)
        .min(SKILL_BUNDLE_RESOLVE_MAX_TIMEOUT.as_secs() as i64);
    Duration::from_secs(scaled_seconds as u64).max(SKILL_BUNDLE_RESOLVE_MIN_TIMEOUT)
}

fn skill_ref_key(source: &str, id: &str) -> String {
    format!("{source}\x00{id}")
}

fn skill_ref_from_bundle(bundle: &SkillData) -> SkillRefData {
    let files = bundle
        .files
        .iter()
        .map(|file| SkillBundleFile {
            path: file.path.clone(),
            content: file.content.clone(),
        })
        .collect();
    let manifest = build_manifest(&SkillBundleSkill {
        id: bundle.id.clone(),
        source: bundle.source.clone(),
        name: bundle.name.clone(),
        description: bundle.description.clone(),
        content: bundle.content.clone(),
        files,
    });
    let file_count = manifest.files.len() as i64;
    let files = manifest
        .files
        .into_iter()
        .map(|file| SkillFileRefData {
            path: file.path,
            sha256: file.sha256,
            size_bytes: file.size_bytes,
        })
        .collect();
    SkillRefData {
        id: bundle.id.clone(),
        source: bundle.source.clone(),
        hash: manifest.hash,
        size_bytes: manifest.size_bytes,
        file_count,
        files,
        ..SkillRefData::default()
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
    use crate::runtime_set::RuntimeSet;
    use crate::types::{Runtime, SkillFileData};
    use cordy_agent::TokenUsage;
    use cordy_protocol::{
        DaemonHeartbeatPendingLocalSkillImport, DaemonHeartbeatPendingLocalSkills,
    };

    struct EnvRestore {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.value.take() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[tokio::test]
    async fn production_heartbeat_lists_and_imports_local_skills_with_retry() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("skills/deploy");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: deploy\ndescription: ship it\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("run.sh"), "echo deploy\n").unwrap();

        let _restore = EnvRestore {
            key: "GROK_HOME",
            value: std::env::var_os("GROK_HOME"),
        };
        unsafe { std::env::set_var("GROK_HOME", temp.path()) };

        let (reports_tx, mut reports_rx) = tokio::sync::mpsc::unbounded_channel();
        let attempts = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let app = {
            let attempts = Arc::clone(&attempts);
            axum::Router::new().fallback(axum::routing::any(
                move |request: axum::extract::Request| {
                    let reports_tx = reports_tx.clone();
                    let attempts = Arc::clone(&attempts);
                    async move {
                        let path = request.uri().path().to_string();
                        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                            .await
                            .unwrap();
                        let payload: Value = serde_json::from_slice(&body).unwrap();
                        reports_tx.send((path.clone(), payload)).unwrap();
                        let mut attempts = attempts.lock().unwrap();
                        let attempt = attempts.entry(path.clone()).or_default();
                        *attempt += 1;
                        if path.ends_with("/local-skills/list-1/result") && *attempt == 1 {
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR
                        } else {
                            axum::http::StatusCode::NO_CONTENT
                        }
                    }
                },
            ))
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let config = Arc::new(Config {
            server_base_url: format!("http://{address}"),
            daemon_id: "daemon-1".to_string(),
            workspaces_root: temp.path().to_string_lossy().into_owned(),
            ..Config::default()
        });
        let adapter = ProductionProviderAdapter::new(config);
        let registry = Arc::new(RuntimeRegistry::new(Arc::new(RuntimeSet::new())));
        registry
            .apply_registration(
                "workspace-1",
                "Workspace",
                vec![Runtime {
                    id: "runtime-1".to_string(),
                    provider: "grok".to_string(),
                    ..Runtime::default()
                }],
            )
            .unwrap();
        let client = Arc::new(Client::new(format!("http://{address}")));
        let launches = Arc::new(RuntimeLaunchRegistry::default());

        adapter
            .handle_non_update_heartbeat_actions(
                Ctx::new(),
                Arc::clone(&registry),
                "runtime-1".to_string(),
                DaemonHeartbeatAckPayload {
                    runtime_id: "runtime-1".to_string(),
                    status: "ok".to_string(),
                    pending_local_skills: Some(DaemonHeartbeatPendingLocalSkills {
                        id: "list-1".to_string(),
                    }),
                    pending_local_skill_import: Some(DaemonHeartbeatPendingLocalSkillImport {
                        id: "ignored-singular".to_string(),
                        skill_key: "deploy".to_string(),
                    }),
                    pending_local_skill_imports: vec![
                        DaemonHeartbeatPendingLocalSkillImport {
                            id: "batch-1".to_string(),
                            skill_key: "deploy".to_string(),
                        },
                        DaemonHeartbeatPendingLocalSkillImport {
                            id: "batch-2".to_string(),
                            skill_key: "deploy".to_string(),
                        },
                    ],
                    server_capabilities: Vec::new(),
                    runtime_gone: false,
                    pending_update: None,
                    pending_model_list: None,
                },
                Arc::clone(&client),
                Arc::clone(&launches),
            )
            .await;
        adapter
            .handle_non_update_heartbeat_actions(
                Ctx::new(),
                Arc::clone(&registry),
                "runtime-1".to_string(),
                DaemonHeartbeatAckPayload {
                    runtime_id: "runtime-1".to_string(),
                    status: "ok".to_string(),
                    pending_local_skill_import: Some(DaemonHeartbeatPendingLocalSkillImport {
                        id: "singular-1".to_string(),
                        skill_key: "deploy".to_string(),
                    }),
                    server_capabilities: Vec::new(),
                    runtime_gone: false,
                    pending_update: None,
                    pending_model_list: None,
                    pending_local_skills: None,
                    pending_local_skill_imports: Vec::new(),
                },
                Arc::clone(&client),
                Arc::clone(&launches),
            )
            .await;

        let mut reports = Vec::new();
        for _ in 0..5 {
            reports.push(
                tokio::time::timeout(Duration::from_secs(3), reports_rx.recv())
                    .await
                    .expect("local skill report timed out")
                    .expect("report server stopped"),
            );
        }
        assert_eq!(
            attempts
                .lock()
                .unwrap()
                .get("/api/daemon/runtimes/runtime-1/local-skills/list-1/result"),
            Some(&2),
            "a transient 500 must retry the list report"
        );
        assert!(reports.iter().all(|(path, _)| !path.contains("ignored-singular")));
        let list = reports
            .iter()
            .find(|(path, _)| path.ends_with("/local-skills/list-1/result"))
            .map(|(_, payload)| payload)
            .unwrap();
        assert_eq!(list["status"], "completed");
        assert_eq!(list["supported"], true);
        assert_eq!(list["skills"][0]["key"], "deploy");
        for id in ["batch-1", "batch-2", "singular-1"] {
            let (_, payload) = reports
                .iter()
                .find(|(path, _)| path.ends_with(&format!("/import/{id}/result")))
                .unwrap();
            assert_eq!(payload["status"], "completed");
            assert_eq!(payload["skill"]["name"], "deploy");
            assert_eq!(payload["skill"]["files"][0]["path"], "run.sh");
        }

        adapter
            .handle_non_update_heartbeat_actions(
                Ctx::new(),
                registry,
                "unknown-runtime".to_string(),
                DaemonHeartbeatAckPayload {
                    runtime_id: "unknown-runtime".to_string(),
                    status: "ok".to_string(),
                    pending_local_skills: Some(DaemonHeartbeatPendingLocalSkills {
                        id: "must-not-report".to_string(),
                    }),
                    server_capabilities: Vec::new(),
                    runtime_gone: false,
                    pending_update: None,
                    pending_model_list: None,
                    pending_local_skill_import: None,
                    pending_local_skill_imports: Vec::new(),
                },
                client,
                launches,
            )
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), reports_rx.recv())
                .await
                .is_err(),
            "unknown runtimes must not execute heartbeat actions"
        );
        server.abort();
    }

    #[tokio::test]
    async fn production_task_rejects_required_remote_mcp_for_incompatible_provider() {
        let temp = tempfile::tempdir().unwrap();
        let config = Arc::new(Config {
            server_base_url: "http://server.invalid".to_string(),
            daemon_id: "daemon-1".to_string(),
            workspaces_root: temp.path().to_string_lossy().into_owned(),
            ..Config::default()
        });
        let adapter = ProductionProviderAdapter::new(config);
        let launches = Arc::new(RuntimeLaunchRegistry::default());
        let target = RuntimeExecutionTarget {
            provider: "deveco".to_string(),
            profile_id: String::new(),
        };
        launches.replace_builtins(
            "workspace-1",
            vec![RuntimeLaunchSpec {
                target: target.clone(),
                display_name: "Deveco".to_string(),
                command_path: "/bin/false".to_string(),
                fixed_args: Vec::new(),
                version: "1".to_string(),
            }],
        );
        let runtime = ProviderRuntimeContext::new(
            Arc::new(Client::new("http://server.invalid")),
            launches,
            crate::activity::DaemonActivity::new(),
            Arc::new(crate::repo_state::DaemonRepoState::new()),
            Arc::new(crate::health::RepoCheckoutRegistry::default()),
        );
        let task = Task {
            id: "task-1".to_string(),
            agent_id: "agent-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            agent: Some(crate::types::AgentData::default()),
            remote_mcp_connections: vec![cordy_remotemcp::Connection {
                contribution_id: "connection-1".to_string(),
                contribution_key: "required-tools".to_string(),
                failure_policy: "required".to_string(),
                ..cordy_remotemcp::Connection::default()
            }],
            ..Task::default()
        };

        let outcome = adapter
            .run_task_inner(Ctx::new(), task, target, 0, runtime)
            .await;
        let failure = outcome.failure.expect("required broker failure");
        assert!(
            failure.message.contains(
                "Remote MCP required-tools is incompatible with provider deveco"
            ),
            "{}",
            failure.message
        );
    }

    #[test]
    fn skill_bundle_timeout_matches_size_budget() {
        assert_eq!(
            skill_bundle_resolve_timeout(0),
            SKILL_BUNDLE_RESOLVE_MIN_TIMEOUT
        );
        assert_eq!(
            skill_bundle_resolve_timeout(2 * 1024 * 1024),
            Duration::from_secs(40)
        );
        assert_eq!(
            skill_bundle_resolve_timeout(100 * 1024 * 1024),
            SKILL_BUNDLE_RESOLVE_MAX_TIMEOUT
        );
    }

    #[test]
    fn skill_ref_from_bundle_rebuilds_the_manifest_identity() {
        let mut bundle = SkillData {
            id: "skill-1".into(),
            source: "workspace".into(),
            name: "deploy".into(),
            content: "main".into(),
            files: vec![SkillFileData {
                path: "rules.md".into(),
                content: "rules".into(),
                ..SkillFileData::default()
            }],
            ..SkillData::default()
        };
        let r#ref = skill_ref_from_bundle(&bundle);
        bundle.hash.clone_from(&r#ref.hash);
        bundle.size_bytes = r#ref.size_bytes;
        bundle.files[0].sha256.clone_from(&r#ref.files[0].sha256);
        bundle.files[0].size_bytes = r#ref.files[0].size_bytes;

        assert!(validate_skill_bundle(&r#ref, &bundle));
        assert!(r#ref.name.is_empty());
        assert_eq!(r#ref.file_count, 1);
        assert_eq!(r#ref.files[0].path, "rules.md");
    }

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
        assert_eq!(batch.tool_use_count, 1);
        batch.begin_attempt();
        batch.push(Message {
            message_type: MessageType::Text,
            content: "fresh".to_string(),
            tool: String::new(),
            call_id: String::new(),
            input: BTreeMap::new(),
            output: String::new(),
            status: String::new(),
            level: String::new(),
            session_id: String::new(),
        });
        assert_eq!(batch.messages[2].seq, 3);
    }

    #[tokio::test]
    async fn codex_rollout_gates_midflight_pin_and_terminal_session_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        let sessions = codex_home.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let rollout = sessions.join("rollout-test-session-delayed.jsonl");
        let codex_home = codex_home.to_string_lossy().into_owned();
        let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = axum::Router::new().fallback(axum::routing::any({
            let rollout = rollout.clone();
            move |request: axum::extract::Request| {
                let requests_tx = requests_tx.clone();
                let rollout = rollout.clone();
                async move {
                    requests_tx
                        .send((request.uri().path().to_string(), rollout.exists()))
                        .unwrap();
                    axum::http::StatusCode::NO_CONTENT
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = Arc::new(Client::new(format!("http://{address}")));

        let (messages_tx, messages_rx) = tokio::sync::mpsc::channel(2);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn({
            let rollout = rollout.clone();
            async move {
                messages_tx
                    .send(Message {
                        message_type: MessageType::Status,
                        content: String::new(),
                        tool: String::new(),
                        call_id: String::new(),
                        input: BTreeMap::new(),
                        output: String::new(),
                        status: "running".to_string(),
                        level: String::new(),
                        session_id: "session-delayed".to_string(),
                    })
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                std::fs::write(rollout, "{}\n").unwrap();
                tokio::time::sleep(Duration::from_millis(80)).await;
                result_tx
                    .send(ExecutionResult {
                        status: "completed".to_string(),
                        session_id: "session-delayed".to_string(),
                        ..ExecutionResult::default()
                    })
                    .unwrap();
            }
        });
        let mut transcript = TranscriptBatch::default();
        let result = drain_session(
            &Ctx::new(),
            &client,
            "task-delayed",
            "/work",
            &codex_home,
            Session {
                messages: messages_rx,
                result: result_rx,
            },
            &mut transcript,
        )
        .await
        .unwrap();
        producer.await.unwrap();
        assert_eq!(result.session_id, "session-delayed");
        let (path, rollout_existed_at_pin) = requests_rx.recv().await.unwrap();
        assert_eq!(path, "/api/daemon/tasks/task-delayed/session");
        assert!(rollout_existed_at_pin);

        let (messages_tx, messages_rx) = tokio::sync::mpsc::channel(2);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        messages_tx
            .send(Message {
                message_type: MessageType::Status,
                content: String::new(),
                tool: String::new(),
                call_id: String::new(),
                input: BTreeMap::new(),
                output: String::new(),
                status: "running".to_string(),
                level: String::new(),
                session_id: "session-missing".to_string(),
            })
            .await
            .unwrap();
        drop(messages_tx);
        result_tx
            .send(ExecutionResult {
                status: "failed".to_string(),
                session_id: "session-missing".to_string(),
                ..ExecutionResult::default()
            })
            .unwrap();
        let mut transcript = TranscriptBatch::default();
        let mut missing = drain_session(
            &Ctx::new(),
            &client,
            "task-missing",
            "/work",
            &codex_home,
            Session {
                messages: messages_rx,
                result: result_rx,
            },
            &mut transcript,
        )
        .await
        .unwrap();
        assert!(requests_rx.try_recv().is_err());
        assert!(
            withhold_missing_codex_rollout(
                &mut missing,
                &codex_home,
                Duration::from_millis(5),
            )
            .await
        );
        assert!(missing.session_id.is_empty());

        let mut present = ExecutionResult {
            status: "completed".to_string(),
            session_id: "session-delayed".to_string(),
            ..ExecutionResult::default()
        };
        assert!(
            !withhold_missing_codex_rollout(&mut present, &codex_home, Duration::from_secs(1))
                .await
        );
        assert_eq!(present.session_id, "session-delayed");

        let mut non_codex = ExecutionResult {
            status: "completed".to_string(),
            session_id: "provider-session".to_string(),
            ..ExecutionResult::default()
        };
        assert!(
            !withhold_missing_codex_rollout(&mut non_codex, "", Duration::ZERO).await
        );
        assert_eq!(non_codex.session_id, "provider-session");
        server.abort();
    }

    #[test]
    fn fresh_retry_requires_resume_failure_without_tool_side_effects() {
        let rejected = ExecutionResult {
            status: "failed".to_string(),
            error: "resume unavailable".to_string(),
            resume_rejected: true,
            ..ExecutionResult::default()
        };
        assert!(should_retry_with_fresh_session(
            &rejected,
            "old-session",
            0,
            "qwen"
        ));
        assert!(!should_retry_with_fresh_session(
            &rejected,
            "old-session",
            1,
            "qwen"
        ));
        assert!(!should_retry_with_fresh_session(
            &rejected, "", 0, "qwen"
        ));

        let broken_history = ExecutionResult {
            status: "failed".to_string(),
            error: "messages[3] assistant message must not be empty".to_string(),
            ..ExecutionResult::default()
        };
        assert!(should_retry_with_fresh_session(
            &broken_history,
            "old-session",
            0,
            "claude"
        ));
        let stale_provider_identity = ExecutionResult {
            status: "failed".to_string(),
            error: "Could not resolve authentication method".to_string(),
            ..ExecutionResult::default()
        };
        assert!(should_retry_with_fresh_session(
            &stale_provider_identity,
            "old-session",
            0,
            "qwen"
        ));

        let undetectable_process_failure = ExecutionResult {
            status: "failed".to_string(),
            error: "process exited before producing a result".to_string(),
            ..ExecutionResult::default()
        };
        assert!(should_retry_with_fresh_session(
            &undetectable_process_failure,
            "old-session",
            0,
            "cursor"
        ));
        let network = ExecutionResult {
            status: "failed".to_string(),
            error: "connection reset by peer".to_string(),
            ..ExecutionResult::default()
        };
        assert!(!should_retry_with_fresh_session(
            &network,
            "old-session",
            0,
            "cursor"
        ));
    }

    #[test]
    fn fresh_retry_never_resurrects_the_abandoned_session_and_merges_usage() {
        let first = ExecutionResult {
            status: "failed".to_string(),
            error: "poisoned history".to_string(),
            session_id: "old-session".to_string(),
            usage: BTreeMap::from([(
                "model".to_string(),
                TokenUsage {
                    input_tokens: 10,
                    ..TokenUsage::default()
                },
            )]),
            ..ExecutionResult::default()
        };
        let retry_without_session = ExecutionResult {
            status: "failed".to_string(),
            error: "fresh launch failed".to_string(),
            usage: BTreeMap::from([(
                "model".to_string(),
                TokenUsage {
                    output_tokens: 3,
                    ..TokenUsage::default()
                },
            )]),
            ..ExecutionResult::default()
        };
        let kept = reconcile_fresh_retry_result(
            first.clone(),
            Ok((retry_without_session, 0)),
        );
        assert_eq!(kept.session_id, "old-session");
        assert_eq!(kept.usage["model"].input_tokens, 10);
        assert_eq!(kept.usage["model"].output_tokens, 3);

        let completed_without_session = ExecutionResult {
            status: "completed".to_string(),
            output: "done".to_string(),
            usage: BTreeMap::from([(
                "model".to_string(),
                TokenUsage {
                    output_tokens: 4,
                    ..TokenUsage::default()
                },
            )]),
            ..ExecutionResult::default()
        };
        let completed = reconcile_fresh_retry_result(
            first,
            Ok((completed_without_session, 0)),
        );
        assert_eq!(completed.status, "completed");
        assert!(completed.session_id.is_empty());
        assert_eq!(completed.usage["model"].input_tokens, 10);
        assert_eq!(completed.usage["model"].output_tokens, 4);
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
    fn poisoned_terminal_shapes_are_classified_and_retired() {
        let env = Environment::default();
        let fallback = result_outcome(
            "claude",
            ExecutionResult {
                status: "completed".to_string(),
                output: "I reached the iteration limit".to_string(),
                session_id: "session-1".to_string(),
                ..ExecutionResult::default()
            },
            &env,
            "",
            "",
        );
        assert_eq!(fallback.result.status, "blocked");
        assert_eq!(fallback.result.failure_reason, "iteration_limit");

        let invalid_history = result_outcome(
            "claude",
            ExecutionResult {
                status: "failed".to_string(),
                error: "API 400 invalid_request_error".to_string(),
                session_id: "session-2".to_string(),
                ..ExecutionResult::default()
            },
            &env,
            "",
            "",
        );
        assert_eq!(invalid_history.result.failure_reason, "api_invalid_request");

        let timeout = result_outcome(
            "codex",
            ExecutionResult {
                status: "timeout".to_string(),
                error: "codex semantic inactivity timeout".to_string(),
                ..ExecutionResult::default()
            },
            &env,
            "session-stuck",
            "",
        );
        assert_eq!(
            timeout.result.failure_reason,
            "codex_semantic_inactivity"
        );

        let overflow = result_outcome(
            "codex",
            ExecutionResult {
                status: "failed".to_string(),
                error: "thread/resume failed: token too long".to_string(),
                ..ExecutionResult::default()
            },
            &env,
            "session-oversized",
            "",
        );
        assert_eq!(overflow.result.failure_reason, "codex_resume_oversized");
        assert_eq!(
            overflow.result.retired_session_id,
            "session-oversized"
        );
    }
}
