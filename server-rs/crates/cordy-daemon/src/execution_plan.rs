//! Pure, fail-closed assembly of one provider execution.
//!
//! This module deliberately stops before environment preparation and process
//! spawn. It converts the claim payload into the exact execenv context, then
//! binds the resulting [`Environment`] into provider options and a child-only
//! environment. Transcript draining, usage, terminal callbacks, and process
//! ownership remain the responsibility of the future `ProviderRuntimeAdapter`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;
use std::time::Duration;

use cordy_agent::ExecOptions;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, TASK_WORKSPACES_ROOT_ENV};
use crate::execenv::codex_shell_env::{
    codex_shell_env_allowlist, ensure_codex_shell_env_policy_config,
};
use crate::execenv::context::RuntimeSkillRefForEnv;
use crate::execenv::execenv::{
    ConnectedApp, Environment, OpenclawGatewayPin, PrepareParams, ProjectResourceForEnv,
    RepoContextForEnv, ReuseParams, SkillContextForEnv, SkillFileContextForEnv, TaskContextForEnv,
};
use crate::execenv::hermes;
use crate::execenv::local_worktree::LocalWorktreeParams;
use crate::openclaw_runtime_config::decode_openclaw_runtime_config;
use crate::prompt::{
    backend_resume_continuity_notice, comment_reply_threads, task_is_squad_leader,
};
use crate::thread_name::derive_task_thread_name_from_task;
use crate::task_execution::InvalidTaskIdentity;
use crate::types::{AgentData, RuntimeExecutionTarget, Task};

const TASK_CONFIG_ROOT_ENV: &str = "CORDY_TASK_CONFIG_ROOT";

/// Non-claim values resolved by the daemon before building a task plan.
///
/// This type intentionally has no `Debug`: MCP config, gateway pins, custom
/// provider environment values, and shell arguments can all contain secrets.
#[derive(Clone, Default)]
pub struct ProviderExecutionInputs {
    pub slot: usize,
    pub temp_dir: String,
    pub default_model: String,
    pub codex_version: String,
    pub openclaw_bin: String,
    pub effective_mcp_config: Option<Value>,
    pub cursor_mcp_auth_source: String,
    pub local_work_dir: String,
    pub local_worktree: Option<LocalWorktreeParams>,
    pub hermes_source_home: String,
    pub hermes_source_must_exist: bool,
    pub hermes_memory_store: String,
    pub hermes_session_store: String,
    /// Accepted runtime fixed arguments used by providers with argv-level
    /// profile selectors.
    pub launch_prefix: Vec<String>,
    pub codex_custom_args: Vec<String>,
    /// Additional daemon-owned child values. Canonical task identity and
    /// credential keys are applied afterwards and therefore cannot be
    /// overridden here.
    pub runtime_env: BTreeMap<String, String>,
    /// Optional daemon-resolved PATH (normally the Cordy binary directory
    /// prepended to the inherited PATH).
    pub path: String,
}

/// Values available only after execenv preparation has completed.
///
/// `system_prompt` is the already-rendered runtime brief for providers that
/// require inline delivery; leave it empty for file-based providers.
#[derive(Clone, Default)]
pub struct PreparedEnvironmentInputs {
    pub system_prompt: String,
    pub openclaw_include_roots: String,
    pub cancellation: CancellationToken,
}

/// A task plan before any filesystem mutation. Clone it to try reuse and then
/// fresh preparation without rebuilding security-sensitive inputs from the
/// original claim.
#[derive(Clone)]
pub struct ProviderExecutionPlan {
    prepare: PrepareParams,
    prior_work_dir: String,
    options: ExecOptionsSeed,
    child_env: ChildEnvironmentSeed,
    launch_prefix: Vec<String>,
}

impl fmt::Debug for ProviderExecutionPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderExecutionPlan")
            .field("task_id", &self.prepare.task_id)
            .field("workspace_id", &self.prepare.workspace_id)
            .field("provider", &self.prepare.provider)
            .field("has_prior_work_dir", &!self.prior_work_dir.is_empty())
            .field("child_env", &self.child_env)
            .finish_non_exhaustive()
    }
}

/// The provider inputs after a concrete workdir/config environment exists.
pub struct BoundProviderExecution {
    pub options: ExecOptions,
    pub child_env: ChildProcessEnvironment,
    pub launch_prefix: Vec<String>,
}

impl fmt::Debug for BoundProviderExecution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundProviderExecution")
            .field("options", &self.options)
            .field("child_env", &self.child_env)
            .finish()
    }
}

/// Environment values destined exclusively for the provider child process.
/// Values never participate in `Debug` output.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ChildProcessEnvironment(BTreeMap<String, String>);

impl ChildProcessEnvironment {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn into_inner(self) -> BTreeMap<String, String> {
        self.0
    }
}

impl fmt::Debug for ChildProcessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildProcessEnvironment")
            .field("variable_count", &self.0.len())
            .finish()
    }
}

#[derive(Clone)]
struct ChildEnvironmentSeed {
    values: BTreeMap<String, String>,
    custom_env: BTreeMap<String, String>,
}

impl fmt::Debug for ChildEnvironmentSeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildEnvironmentSeed")
            .field(
                "variable_count",
                &(self.values.len() + self.custom_env.len()),
            )
            .finish()
    }
}

#[derive(Clone)]
struct ExecOptionsSeed {
    model: String,
    thread_name: String,
    timeout: Duration,
    semantic_inactivity_timeout: Duration,
    first_turn_no_progress_timeout: Duration,
    idle_watchdog_timeout: Duration,
    handshake_timeout: Duration,
    resume_session_id: String,
    resume_continuity_notice: String,
    extra_args: Vec<String>,
    custom_args: Vec<String>,
    mcp_config: Option<Value>,
    thinking_level: String,
    service_tier: String,
    openclaw_mode: String,
}

impl ProviderExecutionPlan {
    pub fn build(
        config: &Config,
        task: &Task,
        target: &RuntimeExecutionTarget,
        inputs: ProviderExecutionInputs,
    ) -> anyhow::Result<Self> {
        let agent = validate_identity(task, target)?;
        let token = task_scoped_auth_token(task)?;
        anyhow::ensure!(
            !config.workspaces_root.trim().is_empty(),
            "invalid execution configuration: missing workspaces root"
        );
        anyhow::ensure!(
            !config.server_base_url.trim().is_empty(),
            "invalid execution configuration: missing server URL"
        );
        anyhow::ensure!(
            !inputs.temp_dir.trim().is_empty(),
            "invalid execution configuration: missing task temp directory"
        );
        validate_env_value("auth_token", &token)?;
        validate_env_value("temp_dir", &inputs.temp_dir)?;

        let provider = target.provider.trim().to_string();
        let custom_env = sanitize_custom_env(agent.custom_env.as_ref())?;
        let mcp_config = inputs
            .effective_mcp_config
            .clone()
            .or_else(|| agent.mcp_config.clone());
        let (openclaw_mode, openclaw_gateway) = if provider == "openclaw" {
            agent
                .runtime_config
                .as_ref()
                .map(decode_openclaw_runtime_config)
                .unwrap_or_default()
        } else {
            (String::new(), OpenclawGatewayPin::default())
        };
        let task_context = task_context(task, agent, &provider);
        let mut extra_args = default_args(config, &provider);
        let mut custom_args = agent.custom_args.clone();
        let mut hermes_source_home = inputs.hermes_source_home;
        let mut hermes_source_must_exist = inputs.hermes_source_must_exist;
        let mut hermes_memory_store = inputs.hermes_memory_store;
        let mut hermes_session_store = inputs.hermes_session_store;
        let mut hermes_env = custom_env.clone();
        let mut launch_prefix = inputs.launch_prefix;
        if provider == "hermes" {
            let selection = hermes::parse_hermes_profile_args(&hermes::hermes_launch_argv(
                &launch_prefix,
                &custom_args,
            ));
            let resolution = hermes::resolve_hermes_profile(
                custom_env
                    .get("HERMES_HOME")
                    .map(String::as_str)
                    .unwrap_or_default(),
                selection.found.then_some(selection.name.as_str()),
            );
            if let Some(error) = resolution.error {
                anyhow::bail!("invalid Hermes profile selection: {error}");
            }
            hermes_source_home = resolution.source_home;
            hermes_source_must_exist = resolution.must_exist;
            hermes_memory_store = hermes::hermes_memory_store_path(
                &config.profile,
                &task.agent_id,
                &hermes_source_home,
            );
            hermes_session_store = hermes::hermes_session_store_path(
                &config.profile,
                &task.agent_id,
                &hermes_source_home,
                &task_context,
            );
            hermes_env.insert("HERMES_HOME".to_string(), hermes_source_home.clone());
            if !task_context.agent_skills.is_empty() {
                let (effective_prefix, effective_custom_args) =
                    hermes::strip_hermes_profile_selectors(&launch_prefix, &custom_args);
                launch_prefix = effective_prefix;
                custom_args = effective_custom_args;
            }
        }
        let codex_custom_args = if inputs.codex_custom_args.is_empty() {
            let mut effective = extra_args.clone();
            effective.extend(agent.custom_args.clone());
            effective
        } else {
            inputs.codex_custom_args.clone()
        };
        // ExecOptions owns its own vectors. Keeping this explicit avoids a
        // later adapter accidentally splicing profile fixed args into the
        // backend-only ExtraArgs region.
        extra_args.shrink_to_fit();

        let prepare = PrepareParams {
            workspaces_root: config.workspaces_root.clone(),
            workspace_id: task.workspace_id.clone(),
            task_id: task.id.clone(),
            agent_name: agent.name.clone(),
            profile: config.profile.clone(),
            provider: provider.clone(),
            codex_version: inputs.codex_version,
            openclaw_bin: inputs.openclaw_bin,
            mcp_config: mcp_config.clone(),
            cursor_mcp_auth_source: inputs.cursor_mcp_auth_source,
            openclaw_gateway,
            local_work_dir: inputs.local_work_dir,
            local_worktree: inputs.local_worktree,
            hermes_source_home,
            hermes_source_must_exist,
            hermes_memory_store,
            hermes_session_store,
            hermes_env: hermes_env.into_iter().collect(),
            reasonix_env: custom_env.clone().into_iter().collect(),
            codex_custom_args,
            task: task_context,
        };

        let mut values = inputs.runtime_env;
        if !inputs.path.is_empty() {
            values.insert("PATH".to_string(), inputs.path);
        }
        for (key, value) in &values {
            validate_env_pair(key, value)?;
        }
        let canonical = [
            ("CORDY_TOKEN", token),
            (TASK_WORKSPACES_ROOT_ENV, config.workspaces_root.clone()),
            ("CORDY_SERVER_URL", config.server_base_url.clone()),
            ("CORDY_DAEMON_PORT", config.health_port.to_string()),
            ("CORDY_WORKSPACE_ID", task.workspace_id.clone()),
            ("CORDY_AGENT_NAME", agent.name.clone()),
            ("CORDY_AGENT_ID", task.agent_id.clone()),
            ("CORDY_TASK_ID", task.id.clone()),
            ("CORDY_TASK_SLOT", inputs.slot.to_string()),
            ("TMPDIR", inputs.temp_dir.clone()),
            ("TMP", inputs.temp_dir.clone()),
            ("TEMP", inputs.temp_dir),
        ];
        for (key, value) in canonical {
            values.insert(key.to_string(), value);
        }
        if !task.autopilot_run_id.is_empty() {
            values.insert(
                "CORDY_AUTOPILOT_RUN_ID".to_string(),
                task.autopilot_run_id.clone(),
            );
        }
        if !task.autopilot_id.is_empty() {
            values.insert("CORDY_AUTOPILOT_ID".to_string(), task.autopilot_id.clone());
        }
        if !task.quick_create_prompt.is_empty() {
            values.insert("CORDY_QUICK_CREATE_TASK_ID".to_string(), task.id.clone());
            if !task.quick_create_attachment_ids.is_empty() {
                values.insert(
                    "CORDY_QUICK_CREATE_ATTACHMENT_IDS".to_string(),
                    serde_json::to_string(&task.quick_create_attachment_ids)?,
                );
            }
        }

        let model = if agent.model.is_empty() {
            inputs.default_model
        } else {
            agent.model.clone()
        };
        Ok(Self {
            prepare,
            prior_work_dir: task.prior_work_dir.clone(),
            options: ExecOptionsSeed {
                model,
                thread_name: derive_task_thread_name_from_task(task),
                timeout: config.agent_timeout,
                semantic_inactivity_timeout: config.codex_semantic_inactivity_timeout,
                first_turn_no_progress_timeout: config.codex_first_turn_no_progress_timeout,
                idle_watchdog_timeout: if provider == "opencode" {
                    config.opencode_idle_watchdog
                } else {
                    Duration::ZERO
                },
                handshake_timeout: config.codex_handshake_timeout,
                resume_session_id: task.prior_session_id.clone(),
                resume_continuity_notice: backend_resume_continuity_notice(task),
                extra_args,
                custom_args,
                mcp_config,
                thinking_level: agent.thinking_level.clone(),
                service_tier: agent.service_tier.clone(),
                openclaw_mode,
            },
            child_env: ChildEnvironmentSeed { values, custom_env },
            launch_prefix,
        })
    }

    pub fn prepare_params(&self) -> PrepareParams {
        self.prepare.clone()
    }

    pub fn task_context(&self) -> &TaskContextForEnv {
        &self.prepare.task
    }

    pub fn prior_work_dir(&self) -> &str {
        &self.prior_work_dir
    }

    pub fn resume_session_id(&self) -> &str {
        &self.options.resume_session_id
    }

    /// Projects the same security-sensitive inputs onto the reuse path.
    /// Keeping this conversion beside [`prepare_params`](Self::prepare_params)
    /// prevents the runtime adapter from rebuilding a subtly different MCP,
    /// provider overlay, or task-context view for follow-up turns.
    pub fn reuse_params(&self, work_dir: impl Into<String>) -> ReuseParams {
        ReuseParams {
            workspaces_root: self.prepare.workspaces_root.clone(),
            work_dir: work_dir.into(),
            provider: self.prepare.provider.clone(),
            codex_version: self.prepare.codex_version.clone(),
            resume_session_id: self.options.resume_session_id.clone(),
            openclaw_bin: self.prepare.openclaw_bin.clone(),
            mcp_config: self.prepare.mcp_config.clone(),
            cursor_mcp_auth_source: self.prepare.cursor_mcp_auth_source.clone(),
            openclaw_gateway: self.prepare.openclaw_gateway.clone(),
            profile: self.prepare.profile.clone(),
            local_directory: false,
            hermes_source_home: self.prepare.hermes_source_home.clone(),
            hermes_source_must_exist: self.prepare.hermes_source_must_exist,
            hermes_env: self.prepare.hermes_env.clone(),
            hermes_memory_store: self.prepare.hermes_memory_store.clone(),
            hermes_session_store: self.prepare.hermes_session_store.clone(),
            reasonix_env: self.prepare.reasonix_env.clone(),
            codex_custom_args: self.prepare.codex_custom_args.clone(),
            task: self.prepare.task.clone(),
        }
    }

    /// Drops a server-provided resume pointer after the adapter proves its
    /// daemon-owned workdir cannot be reused. The continuity-loss signal is
    /// updated in the same operation so both the refreshed runtime files and
    /// the provider options describe a fresh session truthfully.
    pub fn drop_resume(&mut self) {
        if self.options.resume_session_id.is_empty() {
            return;
        }
        self.options.resume_session_id.clear();
        self.options.resume_continuity_notice.clear();
        self.prepare.task.prior_session_resumed = false;
        self.prepare.task.prior_session_resume_unavailable = true;
    }

    pub fn bind_environment(
        &self,
        environment: &Environment,
        prepared: PreparedEnvironmentInputs,
    ) -> anyhow::Result<BoundProviderExecution> {
        anyhow::ensure!(
            !environment.work_dir.trim().is_empty(),
            "provider execution environment has no workdir"
        );
        anyhow::ensure!(
            !environment.cordy_config_root.trim().is_empty(),
            "provider execution environment has no task config root"
        );
        if self.prepare.provider == "codex" {
            anyhow::ensure!(
                !environment.codex_home.trim().is_empty(),
                "codex execution environment has no task CODEX_HOME"
            );
        }

        let mut values = self.child_env.values.clone();
        for (key, value) in &self.child_env.custom_env {
            values.insert(key.clone(), value.clone());
        }
        values.insert(
            TASK_CONFIG_ROOT_ENV.to_string(),
            environment.cordy_config_root.clone(),
        );
        insert_nonempty(&mut values, "CODEX_HOME", &environment.codex_home);
        insert_nonempty(&mut values, "CURSOR_DATA_DIR", &environment.cursor_data_dir);
        insert_nonempty(
            &mut values,
            "OPENCLAW_CONFIG_PATH",
            &environment.openclaw_config_path,
        );
        if !prepared.openclaw_include_roots.trim().is_empty() {
            let mut roots = vec![std::path::PathBuf::from(
                prepared.openclaw_include_roots.as_str(),
            )];
            if let Some(existing) = values.get("OPENCLAW_INCLUDE_ROOTS") {
                roots.extend(std::env::split_paths(existing));
            }
            let joined = std::env::join_paths(roots)
                .map(|joined| joined.to_string_lossy().into_owned())
                .unwrap_or_else(|_| prepared.openclaw_include_roots.clone());
            values.insert("OPENCLAW_INCLUDE_ROOTS".to_string(), joined);
        }
        // The prepared overlay is authoritative over custom_env.
        insert_nonempty(&mut values, "HERMES_HOME", &environment.hermes_home);

        Ok(BoundProviderExecution {
            options: ExecOptions {
                cwd: environment.work_dir.clone(),
                model: self.options.model.clone(),
                system_prompt: prepared.system_prompt,
                thread_name: self.options.thread_name.clone(),
                timeout: self.options.timeout,
                semantic_inactivity_timeout: self.options.semantic_inactivity_timeout,
                first_turn_no_progress_timeout: self.options.first_turn_no_progress_timeout,
                idle_watchdog_timeout: self.options.idle_watchdog_timeout,
                handshake_timeout: self.options.handshake_timeout,
                resume_session_id: self.options.resume_session_id.clone(),
                resume_expected: !self.options.resume_session_id.is_empty(),
                resume_continuity_notice: self.options.resume_continuity_notice.clone(),
                extra_args: self.options.extra_args.clone(),
                custom_args: self.options.custom_args.clone(),
                qwenpaw_workspace: environment.qwenpaw_workspace.clone(),
                mcp_config: self.options.mcp_config.clone(),
                thinking_level: self.options.thinking_level.clone(),
                service_tier: self.options.service_tier.clone(),
                openclaw_mode: self.options.openclaw_mode.clone(),
                claude_settings_path: environment.claude_settings_path.clone(),
                cancellation: prepared.cancellation,
                ..ExecOptions::default()
            },
            child_env: ChildProcessEnvironment(values),
            launch_prefix: self.launch_prefix.clone(),
        })
    }

    /// Writes Codex's task-local shell environment policy after the complete
    /// child environment has been assembled. The policy must be derived from
    /// this exact bound environment: omitting daemon-provided task variables
    /// drops the task CLI credentials, while allowing inherited secret names
    /// exposes daemon credentials to Codex shell tools.
    pub fn configure_codex_shell_environment(
        &self,
        bound: &BoundProviderExecution,
    ) -> anyhow::Result<()> {
        if self.prepare.provider != "codex" {
            return Ok(());
        }
        let codex_home = bound.child_env.get("CODEX_HOME").unwrap_or_default();
        anyhow::ensure!(
            !codex_home.trim().is_empty(),
            "configure Codex shell environment: task CODEX_HOME is missing"
        );

        let inherited = std::env::vars_os()
            .filter_map(|(key, _)| key.into_string().ok())
            .map(|key| format!("{key}="))
            .collect::<Vec<_>>();
        let explicit = bound
            .child_env
            .0
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();
        let authorized_explicit: Vec<String> = self.child_env.custom_env.keys().cloned().collect();
        let include_only =
            codex_shell_env_allowlist(&inherited, &explicit, &authorized_explicit);
        ensure_codex_shell_env_policy_config(
            &Path::new(codex_home).join("config.toml").to_string_lossy(),
            &include_only,
        )
        .map_err(|error| anyhow::anyhow!("configure Codex shell environment: {error:#}"))
    }
}

fn validate_identity<'a>(
    task: &'a Task,
    target: &RuntimeExecutionTarget,
) -> anyhow::Result<&'a AgentData> {
    for (name, value) in [
        ("task id", task.id.as_str()),
        ("workspace id", task.workspace_id.as_str()),
        ("runtime id", task.runtime_id.as_str()),
        ("agent id", task.agent_id.as_str()),
        ("provider", target.provider.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(invalid_task_identity(format!(
                "invalid task identity: missing {name}"
            )));
        }
    }
    let agent = task
        .agent
        .as_ref()
        .ok_or_else(|| invalid_task_identity("invalid task identity: missing agent payload"))?;
    if agent.id != task.agent_id {
        return Err(invalid_task_identity(
            "invalid task identity: agent payload does not match authoritative agent_id",
        ));
    }
    if agent.name.trim().is_empty() {
        return Err(invalid_task_identity(
            "invalid task identity: missing agent name",
        ));
    }
    if task.issue_id.trim().is_empty()
        && task.chat_session_id.trim().is_empty()
        && task.autopilot_run_id.trim().is_empty()
        && task.quick_create_prompt.trim().is_empty()
    {
        return Err(invalid_task_identity(
            "invalid task identity: missing issue, chat, autopilot, or quick-create identity",
        ));
    }
    Ok(agent)
}

fn invalid_task_identity(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(InvalidTaskIdentity).context(message.into())
}

fn task_scoped_auth_token(task: &Task) -> anyhow::Result<String> {
    let token = task.auth_token.trim();
    anyhow::ensure!(
        !token.is_empty(),
        "server did not provide task-scoped auth token"
    );
    anyhow::ensure!(
        token.starts_with("mat_"),
        "server provided non-task-scoped auth token"
    );
    Ok(token.to_string())
}

fn task_context(task: &Task, agent: &AgentData, provider: &str) -> TaskContextForEnv {
    TaskContextForEnv {
        issue_id: task.issue_id.clone(),
        trigger_comment_id: task.trigger_comment_id.clone(),
        trigger_thread_id: task.trigger_thread_id.clone(),
        comment_reply_targets: comment_reply_threads(task),
        new_comment_count: task.new_comment_count,
        new_comments_since: task.new_comments_since.clone(),
        prior_session_resumed: !task.prior_session_id.is_empty(),
        prior_session_resume_unavailable: task.prior_session_resume_unavailable,
        agent_id: task.agent_id.clone(),
        agent_name: agent.name.clone(),
        agent_instructions: agent.instructions.clone(),
        agent_skills: agent
            .skills
            .iter()
            .map(|skill| SkillContextForEnv {
                name: skill.name.clone(),
                description: skill.description.clone(),
                content: skill.content.clone(),
                files: skill
                    .files
                    .iter()
                    .map(|file| SkillFileContextForEnv {
                        path: file.path.clone(),
                        content: file.content.clone(),
                    })
                    .collect(),
            })
            .collect(),
        disabled_runtime_skills: agent
            .disabled_runtime_skills
            .iter()
            .filter(|skill| skill.runtime_id == task.runtime_id && skill.provider == provider)
            .map(|skill| RuntimeSkillRefForEnv {
                root: skill.root.clone(),
                key: skill.key.clone(),
                name: skill.name.clone(),
                plugin: skill.plugin.clone(),
            })
            .collect(),
        repos: task
            .repos
            .iter()
            .map(|repo| RepoContextForEnv {
                url: repo.url.clone(),
                description: repo.description.clone(),
                reference: repo.ref_.clone(),
            })
            .collect(),
        project_id: task.project_id.clone(),
        project_title: task.project_title.clone(),
        project_description: task.project_description.clone(),
        project_resources: task
            .project_resources
            .iter()
            .map(|resource| ProjectResourceForEnv {
                id: resource.id.clone(),
                resource_type: resource.resource_type.clone(),
                resource_ref: Some(resource.resource_ref.clone()),
                label: resource.label.clone(),
            })
            .collect(),
        chat_session_id: task.chat_session_id.clone(),
        chat_channel_type: task.chat_channel_type.clone(),
        chat_channel_delivers_files: task.chat_channel_delivers_files,
        autopilot_run_id: task.autopilot_run_id.clone(),
        autopilot_id: task.autopilot_id.clone(),
        autopilot_title: task.autopilot_title.clone(),
        autopilot_description: task.autopilot_description.clone(),
        autopilot_source: task.autopilot_source.clone(),
        autopilot_trigger_payload: task
            .autopilot_trigger_payload
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_default(),
        quick_create_prompt: task.quick_create_prompt.clone(),
        handoff_note: task.handoff_note.clone(),
        is_squad_leader: task_is_squad_leader(task),
        workspace_context: task.workspace_context.clone(),
        connected_apps: task
            .connected_apps
            .iter()
            .map(|app| ConnectedApp {
                provider: app.provider.clone(),
                server_name: app.server_name.clone(),
                toolkit_slug: app.toolkit_slug.clone(),
                toolkit_name: app.toolkit_name.clone(),
            })
            .collect(),
        requesting_user_name: task.requesting_user_name.clone(),
        requesting_user_profile_description: task.requesting_user_profile_description.clone(),
        initiator_type: task.initiator_type.clone(),
        initiator_id: task.initiator_id.clone(),
        initiator_name: task.initiator_name.clone(),
        initiator_email: task.initiator_email.clone(),
    }
}

fn default_args(config: &Config, provider: &str) -> Vec<String> {
    match provider {
        "claude" => config.claude_args.clone(),
        "codex" => config.codex_args.clone(),
        "codebuddy" => config.codebuddy_args.clone(),
        "qwen" => config.qwen_args.clone(),
        "qwenpaw" => config.qwenpaw_args.clone(),
        _ => Vec::new(),
    }
}

fn sanitize_custom_env(
    custom: Option<&std::collections::HashMap<String, String>>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for (key, value) in custom.into_iter().flatten() {
        if blocked_custom_env_key(key) {
            continue;
        }
        validate_env_pair(key, value)?;
        result.insert(key.clone(), value.clone());
    }
    Ok(result)
}

fn blocked_custom_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.starts_with("CORDY_")
        || matches!(
            upper.as_str(),
            "HOME"
                | "PATH"
                | "USER"
                | "SHELL"
                | "TERM"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "CODEX_HOME"
                | "REASONIX_STATE_HOME"
                | "CURSOR_DATA_DIR"
                | "CURSOR_MCP_AUTH_SOURCE"
                | "OPENCLAW_CONFIG_PATH"
                | "OPENCLAW_INCLUDE_ROOTS"
        )
}

fn validate_env_pair(key: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !key.is_empty()
            && !key.contains('=')
            && !key.contains('\0')
            && !key.chars().any(|ch| matches!(ch, '*' | '?' | '[' | ']')),
        "invalid child environment variable name"
    );
    validate_env_value("child environment variable", value)
}

fn validate_env_value(name: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!value.contains('\0'), "invalid NUL byte in {name}");
    Ok(())
}

fn insert_nonempty(values: &mut BTreeMap<String, String>, key: &str, value: &str) {
    if !value.is_empty() {
        values.insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ConnectedAppData, DisabledRuntimeSkillData, ProjectResourceData, RepoData, SkillData,
        SkillFileData,
    };

    fn config() -> Config {
        Config {
            server_base_url: "https://cordy.example".to_string(),
            workspaces_root: "/workspaces".to_string(),
            profile: "team".to_string(),
            health_port: 19514,
            agent_timeout: Duration::from_secs(90),
            codex_semantic_inactivity_timeout: Duration::from_secs(30),
            codex_first_turn_no_progress_timeout: Duration::from_secs(12),
            codex_handshake_timeout: Duration::from_secs(7),
            codex_args: vec!["--sandbox".to_string(), "workspace-write".to_string()],
            ..Config::default()
        }
    }

    fn task() -> Task {
        Task {
            id: "task-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            agent_id: "agent-1".to_string(),
            issue_id: "issue-1".to_string(),
            auth_token: "mat_task_secret".to_string(),
            remote_mcp_daemon_token: "daemon-broker-secret".to_string(),
            prior_session_id: "session-1".to_string(),
            prior_work_dir: "/old/workdir".to_string(),
            workspace_context: "workspace rules".to_string(),
            project_id: "project-1".to_string(),
            project_title: "Project".to_string(),
            project_description: "project context".to_string(),
            repos: vec![RepoData {
                url: "https://example/repo.git".to_string(),
                description: "main repo".to_string(),
                ref_: "release".to_string(),
            }],
            project_resources: vec![ProjectResourceData {
                id: "resource-1".to_string(),
                resource_type: "github_repo".to_string(),
                resource_ref: serde_json::json!({"url":"https://example/repo"}),
                label: "source".to_string(),
            }],
            chat_channel_type: "slack".to_string(),
            chat_channel_delivers_files: true,
            autopilot_id: "autopilot-1".to_string(),
            quick_create_attachment_ids: vec!["attachment-1".to_string()],
            connected_apps: vec![ConnectedAppData {
                provider: "composio".to_string(),
                server_name: "composio".to_string(),
                toolkit_slug: "notion".to_string(),
                toolkit_name: "Notion".to_string(),
            }],
            agent: Some(AgentData {
                id: "agent-1".to_string(),
                name: "Builder".to_string(),
                instructions: "Ship carefully".to_string(),
                model: "gpt-5".to_string(),
                thinking_level: "high".to_string(),
                service_tier: "priority".to_string(),
                custom_args: vec!["--agent-flag".to_string(), "secret-arg".to_string()],
                custom_env: Some(std::collections::HashMap::from([
                    ("API_KEY".to_string(), "custom-secret".to_string()),
                    ("CORDY_TOKEN".to_string(), "owner-secret".to_string()),
                    ("PATH".to_string(), "/evil".to_string()),
                ])),
                mcp_config: Some(serde_json::json!({"token":"mcp-secret"})),
                skills: vec![SkillData {
                    name: "review".to_string(),
                    description: "Review code".to_string(),
                    content: "instructions".to_string(),
                    files: vec![SkillFileData {
                        path: "references/checklist.md".to_string(),
                        content: "checklist".to_string(),
                        ..SkillFileData::default()
                    }],
                    ..SkillData::default()
                }],
                disabled_runtime_skills: vec![DisabledRuntimeSkillData {
                    runtime_id: "runtime-1".to_string(),
                    provider: "codex".to_string(),
                    root: "/skills".to_string(),
                    key: "unsafe".to_string(),
                    ..DisabledRuntimeSkillData::default()
                }],
                ..AgentData::default()
            }),
            ..Task::default()
        }
    }

    fn target() -> RuntimeExecutionTarget {
        RuntimeExecutionTarget {
            provider: "codex".to_string(),
            profile_id: String::new(),
        }
    }

    #[test]
    fn rejects_glob_syntax_in_custom_environment_names() {
        for name in ["C*", "C?", "C[0]", "C]"] {
            let custom = std::collections::HashMap::from([(name.to_string(), "value".to_string())]);
            assert!(sanitize_custom_env(Some(&custom)).is_err(), "{name}");
        }
    }

    fn inputs() -> ProviderExecutionInputs {
        ProviderExecutionInputs {
            slot: 3,
            temp_dir: "/tmp/task-1".to_string(),
            default_model: "fallback".to_string(),
            path: "/cordy/bin:/usr/bin".to_string(),
            ..ProviderExecutionInputs::default()
        }
    }

    #[test]
    fn maps_claim_context_and_provider_selection_without_losing_contract_fields() {
        let plan = ProviderExecutionPlan::build(&config(), &task(), &target(), inputs()).unwrap();
        let prepare = plan.prepare_params();
        assert_eq!(prepare.workspace_id, "workspace-1");
        assert_eq!(prepare.task_id, "task-1");
        assert_eq!(prepare.provider, "codex");
        assert_eq!(
            prepare.task.agent_skills[0].files[0].path,
            "references/checklist.md"
        );
        assert_eq!(prepare.task.repos[0].reference, "release");
        assert_eq!(prepare.task.project_resources[0].label, "source");
        assert_eq!(prepare.task.workspace_context, "workspace rules");
        assert_eq!(prepare.task.connected_apps[0].toolkit_slug, "notion");
        assert!(prepare.task.prior_session_resumed);
        assert_eq!(plan.prior_work_dir(), "/old/workdir");
        assert_eq!(plan.resume_session_id(), "session-1");
        assert_eq!(
            prepare.codex_custom_args,
            vec!["--sandbox", "workspace-write", "--agent-flag", "secret-arg"]
        );
    }

    #[test]
    fn hermes_plan_resolves_profile_stores_and_overlay_launch_args() {
        let mut task = task();
        let agent = task.agent.as_mut().unwrap();
        agent
            .custom_env
            .as_mut()
            .unwrap()
            .insert("HERMES_HOME".to_string(), "/tmp/hermes".to_string());
        agent.custom_args = vec![
            "--profile".to_string(),
            "research".to_string(),
            "--profile=ignored".to_string(),
            "--agent-flag".to_string(),
        ];
        let target = RuntimeExecutionTarget {
            provider: "hermes".to_string(),
            profile_id: String::new(),
        };
        let mut inputs = inputs();
        inputs.launch_prefix = vec!["--model".to_string(), "coder".to_string()];

        let plan = ProviderExecutionPlan::build(&config(), &task, &target, inputs).unwrap();
        let prepare = plan.prepare_params();
        assert_eq!(prepare.hermes_source_home, "/tmp/hermes/profiles/research");
        assert!(prepare.hermes_source_must_exist);
        assert_eq!(
            prepare.hermes_env.get("HERMES_HOME").map(String::as_str),
            Some("/tmp/hermes/profiles/research"),
        );
        assert!(prepare
            .hermes_memory_store
            .ends_with("hermes-state/agent-1/research_f71c2089e8cc5449",));
        assert!(prepare
            .hermes_session_store
            .ends_with("hermes-sessions/agent-1/research_f71c2089e8cc5449/issue-1",));

        let bound = plan
            .bind_environment(
                &Environment {
                    work_dir: "/workdir".to_string(),
                    cordy_config_root: "/config".to_string(),
                    ..Environment::default()
                },
                PreparedEnvironmentInputs::default(),
            )
            .unwrap();
        assert_eq!(bound.launch_prefix, vec!["--model", "coder"]);
        assert_eq!(bound.options.custom_args, vec!["--agent-flag"]);
    }

    #[test]
    fn binds_task_token_only_to_child_env_and_preserves_exec_options() {
        let plan = ProviderExecutionPlan::build(&config(), &task(), &target(), inputs()).unwrap();
        let bound = plan
            .bind_environment(
                &Environment {
                    work_dir: "/workspaces/ws/task/workdir".to_string(),
                    cordy_config_root: "/workspaces/ws/task/cordy-config".to_string(),
                    codex_home: "/workspaces/ws/task/codex-home".to_string(),
                    claude_settings_path: "/settings.json".to_string(),
                    qwenpaw_workspace: "/qwenpaw".to_string(),
                    ..Environment::default()
                },
                PreparedEnvironmentInputs {
                    system_prompt: "runtime brief".to_string(),
                    ..PreparedEnvironmentInputs::default()
                },
            )
            .unwrap();

        assert_eq!(bound.child_env.get("CORDY_TOKEN"), Some("mat_task_secret"));
        assert!(
            !bound
                .child_env
                .clone()
                .into_inner()
                .values()
                .any(|value| value == "daemon-broker-secret" || value == "owner-secret"),
            "daemon or blocked owner credentials entered the child environment"
        );
        assert_eq!(bound.child_env.get("API_KEY"), Some("custom-secret"));
        assert_eq!(bound.child_env.get("PATH"), Some("/cordy/bin:/usr/bin"));
        assert_eq!(bound.options.cwd, "/workspaces/ws/task/workdir");
        assert_eq!(bound.options.model, "gpt-5");
        assert_eq!(
            bound.options.custom_args,
            vec!["--agent-flag", "secret-arg"]
        );
        assert_eq!(bound.options.thinking_level, "high");
        assert_eq!(bound.options.service_tier, "priority");
        assert_eq!(bound.options.resume_session_id, "session-1");
        assert!(bound.options.resume_expected);

        let prepare_json = serde_json::to_value(plan.prepare_params()).unwrap();
        assert!(!prepare_json.to_string().contains("mat_task_secret"));
    }

    #[test]
    fn configures_codex_shell_policy_from_the_bound_task_environment() {
        let plan = ProviderExecutionPlan::build(&config(), &task(), &target(), inputs()).unwrap();
        let home = tempfile::tempdir().unwrap();
        let codex_home = home.path().join("codex-home");
        std::fs::create_dir_all(&codex_home).unwrap();
        let bound = plan
            .bind_environment(
                &Environment {
                    work_dir: "/workdir".to_string(),
                    cordy_config_root: "/config".to_string(),
                    codex_home: codex_home.to_string_lossy().into_owned(),
                    ..Environment::default()
                },
                PreparedEnvironmentInputs::default(),
            )
            .unwrap();

        plan.configure_codex_shell_environment(&bound).unwrap();

        let config = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
        assert!(config.contains("shell_environment_policy"));
        assert!(config.contains("API_KEY"));
        assert!(config.contains("CORDY_TOKEN"));
        assert!(!config.contains("owner-secret"));
        assert!(!config.contains("/evil"));
    }

    #[test]
    fn credentials_and_environment_values_are_redacted_from_debug() {
        let plan = ProviderExecutionPlan::build(&config(), &task(), &target(), inputs()).unwrap();
        let plan_debug = format!("{plan:?}");
        for secret in [
            "mat_task_secret",
            "custom-secret",
            "mcp-secret",
            "secret-arg",
        ] {
            assert!(!plan_debug.contains(secret), "plan Debug leaked {secret}");
        }
        let bound = plan
            .bind_environment(
                &Environment {
                    work_dir: "/workdir".to_string(),
                    cordy_config_root: "/config".to_string(),
                    codex_home: "/codex".to_string(),
                    ..Environment::default()
                },
                PreparedEnvironmentInputs::default(),
            )
            .unwrap();
        let bound_debug = format!("{bound:?}");
        for secret in [
            "mat_task_secret",
            "custom-secret",
            "mcp-secret",
            "secret-arg",
        ] {
            assert!(!bound_debug.contains(secret), "bound Debug leaked {secret}");
        }
    }

    #[test]
    fn invalid_identity_or_auth_fails_closed_without_daemon_token_fallback() {
        let cases: [(&str, fn(&mut Task)); 5] = [
            ("missing token", |task: &mut Task| task.auth_token.clear()),
            ("daemon token", |task: &mut Task| {
                task.auth_token = "owner-token".to_string()
            }),
            ("missing workspace", |task: &mut Task| {
                task.workspace_id.clear()
            }),
            ("missing runtime", |task: &mut Task| task.runtime_id.clear()),
            ("mismatched agent", |task: &mut Task| {
                task.agent.as_mut().unwrap().id = "other".to_string()
            }),
        ];
        for (name, mutate) in cases {
            let mut claim = task();
            mutate(&mut claim);
            assert!(
                ProviderExecutionPlan::build(&config(), &claim, &target(), inputs()).is_err(),
                "{name} unexpectedly built a plan"
            );
        }
    }

    #[test]
    fn invalid_identity_keeps_a_structural_failure_sentinel() {
        let mut claim = task();
        claim.agent.as_mut().unwrap().id = "other".to_string();
        let error = match ProviderExecutionPlan::build(&config(), &claim, &target(), inputs()) {
            Ok(_) => panic!("mismatched identity unexpectedly built a plan"),
            Err(error) => error,
        };
        assert!(error.is::<InvalidTaskIdentity>());
    }

    #[test]
    fn chat_autopilot_and_quick_create_markers_are_preserved() {
        let mut claim = task();
        claim.issue_id.clear();
        claim.chat_session_id = "chat-1".to_string();
        claim.chat_message = "hello".to_string();
        claim.autopilot_run_id = "run-1".to_string();
        claim.quick_create_prompt = "create an issue".to_string();
        let plan = ProviderExecutionPlan::build(&config(), &claim, &target(), inputs()).unwrap();
        let ctx = plan.task_context();
        assert_eq!(ctx.chat_session_id, "chat-1");
        assert_eq!(ctx.autopilot_run_id, "run-1");
        assert_eq!(ctx.quick_create_prompt, "create an issue");
        let bound = plan
            .bind_environment(
                &Environment {
                    work_dir: "/workdir".to_string(),
                    cordy_config_root: "/config".to_string(),
                    codex_home: "/codex".to_string(),
                    ..Environment::default()
                },
                PreparedEnvironmentInputs::default(),
            )
            .unwrap();
        assert_eq!(bound.child_env.get("CORDY_AUTOPILOT_RUN_ID"), Some("run-1"));
        assert_eq!(
            bound.child_env.get("CORDY_QUICK_CREATE_TASK_ID"),
            Some("task-1")
        );
        assert_eq!(
            bound.child_env.get("CORDY_QUICK_CREATE_ATTACHMENT_IDS"),
            Some("[\"attachment-1\"]")
        );
    }
}
