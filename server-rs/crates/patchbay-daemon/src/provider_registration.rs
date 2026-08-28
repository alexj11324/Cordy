//! Provider-backed runtime registration and launch identity publication.
//!
//! The daemon owns workspace/profile orchestration while the provider crate
//! owns executable discovery, version policy, and fixed-argument filtering.
//! This module joins those responsibilities without providing a fallback
//! catalog: production construction requires a real [`ProviderCatalog`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};

use crate::agents_refresh::RuntimeVerdict;
use crate::client::{Client, RuntimeProfile};
use crate::config::Config;
use crate::health::AgentHealthSnapshot;
use crate::registration::{
    BuiltinRefreshReason, RegistrationPayload, RuntimeRegistrationRound, RuntimeRegistrationSource,
};
use crate::repocache::Ctx;
use crate::types::RuntimeExecutionTarget;
use patchbay_agent::version::VersionError;
use patchbay_agent::{
    build_backend, check_provider_minimum, extract_version_line, filter_launch_prefix_for_provider,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProbeReason {
    Registration,
    Discovery,
    Version,
}

impl From<BuiltinRefreshReason> for ProviderProbeReason {
    fn from(reason: BuiltinRefreshReason) -> Self {
        match reason {
            BuiltinRefreshReason::Discovery => Self::Discovery,
            BuiltinRefreshReason::Version => Self::Version,
        }
    }
}

/// One launchable provider detected on this machine. `profile_id` is always
/// empty here; workspace custom profiles are resolved separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedProviderRuntime {
    pub provider: String,
    pub display_name: String,
    pub version: String,
    pub command_path: String,
    pub fixed_args: Vec<String>,
}

/// One machine-level provider probe. A missing detected row is not enough to
/// decide whether an existing runtime should be removed: transient failures
/// are preserved, while only confirmed verdicts may enter the demotion path.
#[derive(Debug, Clone, Default)]
pub struct ProviderProbeResult {
    pub detected: Vec<DetectedProviderRuntime>,
    pub demotable: BTreeMap<String, RuntimeVerdict>,
    pub unavailable: BTreeMap<String, String>,
    /// Diagnostic-only reasons for discovered CLIs that cannot participate in
    /// registration. Unlike `unavailable` and `demotable`, this map never
    /// steers preservation or demotion behavior.
    pub skipped: BTreeMap<String, String>,
}

/// Provider-owned resolution of a workspace profile after applying its safe
/// fixed-argument policy and any validated per-machine path override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfileCommand {
    pub command_path: String,
    pub fixed_args: Vec<String>,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct ProfileResolutionError {
    pub reason: String,
    /// A previously accepted launch remains usable while this transient probe
    /// is retried. Permanent policy/path/version failures must set this false.
    pub preserve_existing: bool,
}

/// Mandatory provider-core surface. Failures must be returned as errors;
/// returning an empty successful probe is authoritative and removes vanished
/// built-in runtimes during a refresh.
#[async_trait::async_trait]
pub trait ProviderCatalog: Send + Sync + 'static {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<ProviderProbeResult>;

    async fn resolve_profile(
        &self,
        ctx: Ctx,
        profile: &RuntimeProfile,
        command_override: Option<&str>,
    ) -> Result<ResolvedProfileCommand, ProfileResolutionError>;
}

/// The daemon-owned catalog for locally installed provider CLIs.
///
/// This is deliberately a thin adapter over the two canonical registries:
/// `agents_probe` owns PATH/login-shell discovery and `patchbay-agent` owns the
/// provider-family/backend capability table. It never manufactures a
/// metadata-only backend. Unsupported families are omitted from registration
/// until their real `patchbay-agent` transport lands, while custom profiles carry
/// a structured failure back to the server.
pub struct LocalProviderCatalog {
    version_probe_timeout: Duration,
    not_executable_confirm_window: Duration,
    not_executable_since: Mutex<HashMap<String, Instant>>,
    detected: Mutex<HashMap<String, DetectedProviderRuntime>>,
}

impl Default for LocalProviderCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProviderCatalog {
    const DEFAULT_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
    const DEFAULT_NOT_EXECUTABLE_CONFIRM_WINDOW: Duration = Duration::from_secs(60);
    const VERSION_PROBE_RETRY_DELAY: Duration = Duration::from_millis(500);
    const VERSION_PROBE_RETRY_WINDOW: Duration = Duration::from_secs(1);
    const VERSION_PROBE_ATTEMPTS: usize = 2;

    pub fn new() -> Self {
        Self {
            version_probe_timeout: Self::DEFAULT_VERSION_PROBE_TIMEOUT,
            not_executable_confirm_window: Self::DEFAULT_NOT_EXECUTABLE_CONFIRM_WINDOW,
            not_executable_since: Mutex::new(HashMap::new()),
            detected: Mutex::new(HashMap::new()),
        }
    }

    /// Test/embedding seam for the bounded `--version` process probe. The
    /// production default remains ten seconds, matching the Go daemon's
    /// per-provider probe budget.
    pub fn with_version_probe_timeout(timeout: Duration) -> Self {
        Self {
            version_probe_timeout: timeout,
            ..Self::new()
        }
    }

    #[cfg(test)]
    fn with_probe_windows(timeout: Duration, confirm_window: Duration) -> Self {
        Self {
            version_probe_timeout: timeout,
            not_executable_confirm_window: confirm_window,
            not_executable_since: Mutex::new(HashMap::new()),
            detected: Mutex::new(HashMap::new()),
        }
    }

    fn supports_backend(runtime_id: &str) -> bool {
        build_backend(runtime_id, patchbay_agent::BackendConfig::default()).is_ok()
    }

    fn supports_profile_backend(runtime_id: &str) -> bool {
        // Built-in identities such as `omp` can map to a provider family for
        // execution, but custom profile protocol families must be explicit
        // provider identities.
        patchbay_agent::provider(runtime_id).is_some() && Self::supports_backend(runtime_id)
    }

    fn display_name(provider: &str) -> Option<&'static str> {
        patchbay_agent::provider(provider)
            .map(|descriptor| descriptor.display_name)
            .or_else(|| patchbay_agent::builtin_runtime(provider).map(|runtime| runtime.display_name))
    }

    fn fixed_args(provider: &str) -> Vec<String> {
        // DSH is probed with the Patchbay profile, and the same profile selector
        // must precede its protocol-owned `--version` argument at launch.
        if provider == "dsh" {
            vec!["--profile".to_string(), "patchbay".to_string()]
        } else {
            Vec::new()
        }
    }

    async fn probe_version(
        &self,
        ctx: &Ctx,
        command_path: &str,
        fixed_args: &[String],
    ) -> anyhow::Result<String> {
        let mut command = tokio::process::Command::new(command_path);
        command.args(fixed_args).arg("--version");
        let probe_ctx = ctx.child();
        let output =
            crate::gc::processtree::output(&probe_ctx, command, self.version_probe_timeout);
        tokio::pin!(output);
        let output = tokio::select! {
            result = &mut output => result?,
            () = ctx.cancelled() => {
                let cause = ctx.cause();
                probe_ctx.cancel_with(cause);
                let _ = output.await;
                anyhow::bail!("provider version probe cancelled: {cause}");
            }
            () = tokio::time::sleep(self.version_probe_timeout) => {
                probe_ctx.cancel_with(crate::repocache::CancelCause::DeadlineExceeded);
                let cleanup = output.await;
                if let Err(error) = cleanup {
                    tracing::debug!(%command_path, %error, "provider version probe process tree stopped after deadline");
                }
                anyhow::bail!(
                    "provider version probe timed out after {:?}",
                    self.version_probe_timeout
                );
            }
        };
        Ok(extract_version_line(&String::from_utf8_lossy(&output)))
    }

    fn confirm_not_executable(&self, provider: &str, now: Instant) -> bool {
        let mut first_seen = self.not_executable_since.lock().unwrap();
        let Some(first) = first_seen.get(provider) else {
            first_seen.insert(provider.to_string(), now);
            return false;
        };
        now.duration_since(*first) >= self.not_executable_confirm_window
    }

    fn clear_not_executable(&self, provider: &str) {
        self.not_executable_since.lock().unwrap().remove(provider);
    }

    #[allow(clippy::result_large_err)]
    async fn probe_builtin(
        &self,
        ctx: &Ctx,
        provider: &str,
        display_name: &str,
        command_path: &str,
        fixed_args: &[String],
    ) -> Result<DetectedProviderRuntime, ProbeFailure> {
        let mut last_error = None;
        for attempt in 0..Self::VERSION_PROBE_ATTEMPTS {
            if attempt > 0 {
                tokio::select! {
                    () = ctx.cancelled() => break,
                    () = tokio::time::sleep(Self::VERSION_PROBE_RETRY_DELAY) => {}
                }
                if ctx.err().is_some() {
                    break;
                }
            }
            let started = Instant::now();
            match self.probe_version(ctx, command_path, fixed_args).await {
                Ok(version) => match check_provider_minimum(provider, &version) {
                    Ok(()) => {
                        self.clear_not_executable(provider);
                        return Ok(DetectedProviderRuntime {
                            provider: provider.to_string(),
                            display_name: display_name.to_string(),
                            version,
                            command_path: command_path.to_string(),
                            fixed_args: fixed_args.to_vec(),
                        });
                    }
                    Err(VersionError::TooOld) => {
                        return Err(ProbeFailure::Demotable(RuntimeVerdict {
                            reason: format!(
                                "provider version {version:?} is below the required minimum"
                            ),
                            offline: None,
                        }));
                    }
                    Err(error) => last_error = Some(anyhow::Error::new(error)),
                },
                Err(error) => {
                    if is_exec_format_error(&error) {
                        let reason = format!("agent CLI is not executable: {error:#}");
                        if self.confirm_not_executable(provider, Instant::now()) {
                            return Err(ProbeFailure::Demotable(RuntimeVerdict {
                                reason: reason.clone(),
                                offline: Some(crate::client::RuntimeOfflineReason {
                                    code: crate::client::RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE
                                        .to_string(),
                                    detail: reason,
                                    repair: exec_format_repair(command_path),
                                }),
                            }));
                        }
                        return Err(ProbeFailure::Unavailable(reason));
                    }
                    last_error = Some(error);
                }
            }
            if started.elapsed() >= Self::VERSION_PROBE_RETRY_WINDOW {
                break;
            }
        }
        Err(ProbeFailure::Unavailable(
            last_error
                .map(|error| format!("version detection failed: {error:#}"))
                .unwrap_or_else(|| "version detection failed".to_string()),
        ))
    }

    async fn resolve_command(
        &self,
        ctx: &Ctx,
        profile: &RuntimeProfile,
        command_override: Option<&str>,
    ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
        if !Self::supports_profile_backend(&profile.protocol_family) {
            return Err(ProfileResolutionError {
                reason: format!("unsupported protocol family: {}", profile.protocol_family),
                preserve_existing: false,
            });
        }

        let command_path = command_override
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .filter(|path| crate::config::agent_executable_present(path))
            .map(ToOwned::to_owned)
            .or_else(|| crate::config::resolve_agent_executable_path(&profile.command_name).ok())
            .ok_or_else(|| ProfileResolutionError {
                reason: format!("runtime command not executable: {}", profile.command_name),
                preserve_existing: false,
            })?;

        let fixed_args =
            filter_launch_prefix_for_provider(&profile.protocol_family, &profile.fixed_args);
        let version = self
            .probe_version(ctx, &command_path, &fixed_args)
            .await
            .map_err(|error| ProfileResolutionError {
                reason: format!("provider version probe failed: {error}"),
                preserve_existing: true,
            })?;
        check_provider_minimum(&profile.protocol_family, &version).map_err(|error| {
            ProfileResolutionError {
                reason: format!("provider version {version:?} is not supported: {error}"),
                preserve_existing: false,
            }
        })?;
        Ok(ResolvedProfileCommand {
            command_path,
            fixed_args,
            version,
        })
    }
}

enum ProbeFailure {
    Unavailable(String),
    Demotable(RuntimeVerdict),
}

fn is_exec_format_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error)
            .is_some_and(|code| {
                #[cfg(windows)]
                {
                    code == 193
                }
                #[cfg(not(windows))]
                {
                    code == 8
                }
            })
    })
}

fn exec_format_repair(exec_path: &str) -> Option<crate::client::Repair> {
    let path = Path::new(exec_path);
    let bin = path.parent()?;
    if !bin
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("bin")
    {
        return None;
    }
    let root = bin.parent()?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("package.json")).ok()?).ok()?;
    let script = manifest
        .get("scripts")?
        .get("postinstall")?
        .as_str()?
        .trim();
    if script.is_empty() {
        return None;
    }
    let package = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })?;
    let root = root.to_string_lossy();
    #[cfg(windows)]
    let (command, shell) = (
        format!("Set-Location '{}'\n{script}", root.replace('\'', "''")),
        "powershell",
    );
    #[cfg(not(windows))]
    let (command, shell) = (
        format!("cd '{}' && {script}", root.replace('\'', "'\\''")),
        "bash",
    );
    Some(crate::client::Repair {
        package,
        command,
        shell: shell.to_string(),
    })
}

#[async_trait::async_trait]
impl ProviderCatalog for LocalProviderCatalog {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<ProviderProbeResult> {
        let agents = crate::agents_probe::probe_agent_clis();
        let mut result = ProviderProbeResult::default();
        let mut discovered = BTreeSet::new();
        let mut probes = FuturesUnordered::new();
        for (provider, entry) in agents {
            if !Self::supports_backend(&provider) {
                let reason = "provider CLI has no Rust backend".to_string();
                tracing::debug!(%provider, %reason, "withholding provider registration");
                result.skipped.insert(provider, reason);
                continue;
            }
            let Some(display_name) = Self::display_name(&provider) else {
                let reason = "provider CLI has no catalog metadata".to_string();
                tracing::warn!(%provider, %reason, "withholding provider registration");
                result.skipped.insert(provider, reason);
                continue;
            };
            let fixed_args = Self::fixed_args(&provider);
            discovered.insert(provider.clone());
            let cached = self.detected.lock().unwrap().get(&provider).cloned();
            if reason == ProviderProbeReason::Discovery
                && cached.as_ref().is_some_and(|runtime| {
                    runtime.command_path == entry.path && runtime.fixed_args == fixed_args
                })
            {
                result
                    .detected
                    .push(cached.expect("cached runtime was checked"));
                continue;
            }
            let probe_ctx = ctx.child();
            probes.push(async move {
                let outcome = self
                    .probe_builtin(
                        &probe_ctx,
                        &provider,
                        display_name,
                        &entry.path,
                        &fixed_args,
                    )
                    .await;
                (provider, outcome)
            });
        }
        self.detected
            .lock()
            .unwrap()
            .retain(|provider, _| discovered.contains(provider));
        while let Some((provider, outcome)) = probes.next().await {
            match outcome {
                Ok(runtime) => result.detected.push(runtime),
                Err(ProbeFailure::Unavailable(reason)) => {
                    tracing::debug!(%provider, %reason, "provider version probe unavailable; preserving accepted runtime");
                    result.unavailable.insert(provider, reason);
                }
                Err(ProbeFailure::Demotable(verdict)) => {
                    self.detected.lock().unwrap().remove(&provider);
                    tracing::warn!(%provider, reason = %verdict.reason, "provider CLI is confirmed unusable; scheduling runtime demotion");
                    result.demotable.insert(provider, verdict);
                }
            }
        }
        for runtime in &result.detected {
            self.detected
                .lock()
                .unwrap()
                .insert(runtime.provider.clone(), runtime.clone());
        }
        Ok(result)
    }

    async fn resolve_profile(
        &self,
        ctx: Ctx,
        profile: &RuntimeProfile,
        command_override: Option<&str>,
    ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
        self.resolve_command(&ctx, profile, command_override).await
    }
}

/// Exact machine command selected for a runtime registration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunchSpec {
    pub target: RuntimeExecutionTarget,
    pub display_name: String,
    pub command_path: String,
    pub fixed_args: Vec<String>,
    pub version: String,
}

#[derive(Default)]
struct LaunchState {
    builtins: HashMap<String, BTreeMap<String, RuntimeLaunchSpec>>,
    profiles: HashMap<String, BTreeMap<String, RuntimeLaunchSpec>>,
}

/// Authoritative daemon-local launch registry. Built-ins and custom profiles
/// are published per workspace only after that workspace accepts the matching
/// registration response. Failed registrations leave its last accepted launch
/// set untouched, including when another workspace advances successfully.
#[derive(Default)]
pub struct RuntimeLaunchRegistry {
    state: RwLock<LaunchState>,
}

impl RuntimeLaunchRegistry {
    #[cfg(test)]
    pub(crate) fn replace_builtins(&self, workspace_id: &str, specs: Vec<RuntimeLaunchSpec>) {
        let builtins = specs
            .into_iter()
            .map(|spec| (spec.target.provider.clone(), spec))
            .collect();
        self.state
            .write()
            .unwrap()
            .builtins
            .insert(workspace_id.to_string(), builtins);
    }

    fn apply_builtins(
        &self,
        workspace_id: &str,
        specs: &[RuntimeLaunchSpec],
        accepted_providers: &BTreeSet<String>,
    ) {
        let mut state = self.state.write().unwrap();
        let builtins = state.builtins.entry(workspace_id.to_string()).or_default();
        builtins.retain(|provider, _| accepted_providers.contains(provider));
        for spec in specs {
            if accepted_providers.contains(&spec.target.provider) {
                builtins.insert(spec.target.provider.clone(), spec.clone());
            }
        }
    }

    fn remove_builtins(&self, workspace_id: &str, providers: &BTreeSet<String>) {
        let mut state = self.state.write().unwrap();
        if let Some(builtins) = state.builtins.get_mut(workspace_id) {
            builtins.retain(|provider, _| !providers.contains(provider));
        }
    }

    pub(crate) fn replace_workspace_profiles(
        &self,
        workspace_id: &str,
        specs: Vec<RuntimeLaunchSpec>,
    ) {
        let profiles = specs
            .into_iter()
            .map(|spec| (spec.target.profile_id.clone(), spec))
            .collect();
        self.state
            .write()
            .unwrap()
            .profiles
            .insert(workspace_id.to_string(), profiles);
    }

    fn workspace_profiles(&self, workspace_id: &str) -> Vec<RuntimeLaunchSpec> {
        self.state
            .read()
            .unwrap()
            .profiles
            .get(workspace_id)
            .map(|profiles| profiles.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn remove_workspace(&self, workspace_id: &str) {
        let mut state = self.state.write().unwrap();
        state.builtins.remove(workspace_id);
        state.profiles.remove(workspace_id);
    }

    #[cfg(test)]
    fn builtin_refresh_needed(&self, workspace_id: &str, incoming: &[RuntimeLaunchSpec]) -> bool {
        self.builtin_refresh_needed_preserving(workspace_id, incoming, &BTreeSet::new())
    }

    fn builtin_refresh_needed_preserving(
        &self,
        workspace_id: &str,
        incoming: &[RuntimeLaunchSpec],
        preserved: &BTreeSet<String>,
    ) -> bool {
        let state = self.state.read().unwrap();
        let Some(current) = state.builtins.get(workspace_id) else {
            return true;
        };
        current
            .keys()
            .filter(|provider| !preserved.contains(provider.as_str()))
            .count()
            != incoming.len()
            || incoming.iter().any(|spec| {
                current
                    .get(&spec.target.provider)
                    .is_none_or(|saved| saved != spec)
            })
    }

    pub fn resolve(
        &self,
        workspace_id: &str,
        target: &RuntimeExecutionTarget,
    ) -> Option<RuntimeLaunchSpec> {
        let state = self.state.read().unwrap();
        if target.profile_id.is_empty() {
            return state
                .builtins
                .get(workspace_id)
                .and_then(|builtins| builtins.get(&target.provider))
                .cloned();
        }
        state
            .profiles
            .get(workspace_id)
            .and_then(|profiles| profiles.get(&target.profile_id))
            .filter(|spec| spec.target.provider == target.provider)
            .cloned()
    }
}

pub struct ProviderRegistrationSource<C: ProviderCatalog> {
    config: Arc<Config>,
    client: Arc<Client>,
    catalog: Arc<C>,
    launches: Arc<RuntimeLaunchRegistry>,
    agent_health: RwLock<AgentHealthSnapshot>,
}

impl<C: ProviderCatalog> ProviderRegistrationSource<C> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        catalog: Arc<C>,
        launches: Arc<RuntimeLaunchRegistry>,
    ) -> Self {
        let agent_health = AgentHealthSnapshot {
            agents: config.agents.keys().cloned().collect(),
            skipped_agents: HashMap::new(),
        };
        Self {
            config,
            client,
            catalog,
            launches,
            agent_health: RwLock::new(agent_health),
        }
    }

    async fn probe(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
        sampled_after_demotion_seq: u64,
    ) -> anyhow::Result<BuiltinSnapshot> {
        let probe = self.catalog.probe_builtins(ctx, reason).await?;
        let agent_health = agent_health_snapshot_from_probe(&probe);
        let mut detected = probe.detected;
        detected.sort_by(|left, right| left.provider.cmp(&right.provider));
        let mut payload = Vec::with_capacity(detected.len());
        let mut launches = Vec::with_capacity(detected.len());
        let mut previous = None;
        for runtime in detected {
            anyhow::ensure!(!runtime.provider.is_empty(), "detected provider is empty");
            anyhow::ensure!(
                !runtime.command_path.is_empty(),
                "detected provider {} has no executable path",
                runtime.provider
            );
            anyhow::ensure!(
                previous.as_deref() != Some(runtime.provider.as_str()),
                "detected provider {} was returned more than once",
                runtime.provider
            );
            previous = Some(runtime.provider.clone());
            let name = display_name(&runtime.display_name, &self.config.device_name);
            payload.push(registration_entry(
                name.clone(),
                runtime.provider.clone(),
                runtime.version.clone(),
                None,
            ));
            launches.push(RuntimeLaunchSpec {
                target: RuntimeExecutionTarget {
                    provider: runtime.provider,
                    profile_id: String::new(),
                },
                display_name: name,
                command_path: runtime.command_path,
                fixed_args: runtime.fixed_args,
                version: runtime.version,
            });
        }
        let recovered = launches
            .iter()
            .map(|launch| launch.target.provider.clone())
            .collect();
        *self.agent_health.write().unwrap() = agent_health;
        Ok(BuiltinSnapshot {
            payload,
            launches,
            demotable: probe.demotable,
            unavailable: probe.unavailable.into_keys().collect(),
            recovered,
            sampled_after_demotion_seq,
        })
    }
}

struct BuiltinSnapshot {
    payload: Vec<BTreeMap<String, String>>,
    launches: Vec<RuntimeLaunchSpec>,
    demotable: BTreeMap<String, RuntimeVerdict>,
    unavailable: BTreeSet<String>,
    recovered: BTreeSet<String>,
    sampled_after_demotion_seq: u64,
}

struct ProviderRegistrationRound<C: ProviderCatalog> {
    config: Arc<Config>,
    client: Arc<Client>,
    catalog: Arc<C>,
    launches: Arc<RuntimeLaunchRegistry>,
    builtins: Vec<BTreeMap<String, String>>,
    builtin_launches: Vec<RuntimeLaunchSpec>,
    demotable: BTreeMap<String, RuntimeVerdict>,
    unavailable: BTreeSet<String>,
    recovered: BTreeSet<String>,
    sampled_after_demotion_seq: u64,
    include_profiles: bool,
    pending_profiles: Mutex<HashMap<String, Vec<RuntimeLaunchSpec>>>,
}

#[async_trait::async_trait]
impl<C: ProviderCatalog> RuntimeRegistrationRound for ProviderRegistrationRound<C> {
    async fn payload_for_workspace(
        &self,
        ctx: Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RegistrationPayload> {
        let mut payload = RegistrationPayload {
            runtimes: self.builtins.clone(),
            failed_profiles: Vec::new(),
        };
        if !self.include_profiles {
            return Ok(payload);
        }
        let profiles = match self.client.get_runtime_profiles(&ctx, workspace_id).await {
            Ok(response) => response.runtime_profiles,
            Err(error) => {
                tracing::info!(%workspace_id, %error, "custom runtime profile fetch failed; preserving prior launch set");
                return Err(anyhow::anyhow!(
                    "fetch custom runtime profiles for workspace {workspace_id}: {error}"
                ));
            }
        };
        let existing_profiles: BTreeMap<String, RuntimeLaunchSpec> = self
            .launches
            .workspace_profiles(workspace_id)
            .into_iter()
            .map(|spec| (spec.target.profile_id.clone(), spec))
            .collect();
        let mut launches = Vec::new();
        for profile in profiles {
            if !profile.enabled {
                continue;
            }
            if profile.id.is_empty()
                || profile.command_name.is_empty()
                || profile.protocol_family.is_empty()
                || (!profile.workspace_id.is_empty() && profile.workspace_id != workspace_id)
            {
                payload.failed_profiles.push(profile_failure(
                    &profile,
                    "invalid runtime profile identity or command",
                ));
                continue;
            }
            if !LocalProviderCatalog::supports_profile_backend(&profile.protocol_family) {
                payload
                    .failed_profiles
                    .push(profile_failure(&profile, "unsupported protocol family"));
                continue;
            }
            let command_override = self
                .config
                .profile_command_overrides
                .get(&profile.id)
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty());
            match self
                .catalog
                .resolve_profile(ctx.child(), &profile, command_override)
                .await
            {
                Ok(command) if command.command_path.is_empty() => {
                    payload.failed_profiles.push(profile_failure(
                        &profile,
                        "resolved runtime profile has no executable path",
                    ));
                }
                Ok(command) => {
                    let fixed_args = filter_launch_prefix_for_provider(
                        &profile.protocol_family,
                        &command.fixed_args,
                    );
                    let target = RuntimeExecutionTarget {
                        provider: profile.protocol_family.clone(),
                        profile_id: profile.id.clone(),
                    };
                    let name = display_name(&profile.display_name, &self.config.device_name);
                    payload.runtimes.push(registration_entry(
                        name.clone(),
                        target.provider.clone(),
                        command.version.clone(),
                        Some(target.profile_id.clone()),
                    ));
                    launches.push(RuntimeLaunchSpec {
                        target,
                        display_name: name,
                        command_path: command.command_path,
                        fixed_args,
                        version: command.version,
                    });
                }
                Err(error) => {
                    let preserved = error.preserve_existing.then(|| {
                        existing_profiles
                            .get(&profile.id)
                            .filter(|spec| spec.target.provider == profile.protocol_family)
                            .cloned()
                    });
                    if let Some(spec) = preserved.flatten() {
                        tracing::debug!(
                            workspace_id = %workspace_id,
                            profile_id = %profile.id,
                            reason = %error.reason,
                            "custom runtime profile probe unavailable; preserving accepted launch"
                        );
                        payload.runtimes.push(registration_entry(
                            spec.display_name.clone(),
                            spec.target.provider.clone(),
                            spec.version.clone(),
                            Some(spec.target.profile_id.clone()),
                        ));
                        launches.push(spec);
                    } else {
                        payload
                            .failed_profiles
                            .push(profile_failure(&profile, &error.reason));
                    }
                }
            }
        }
        self.pending_profiles
            .lock()
            .unwrap()
            .insert(workspace_id.to_string(), launches);
        Ok(payload)
    }

    fn builtin_registration_needed(&self, workspace_id: &str) -> bool {
        self.launches.builtin_refresh_needed_preserving(
            workspace_id,
            &self.builtin_launches,
            &self.unavailable,
        )
    }

    fn sampled_after_demotion_seq(&self) -> u64 {
        self.sampled_after_demotion_seq
    }

    fn recovered_providers(&self) -> BTreeSet<String> {
        self.recovered.clone()
    }

    fn preserved_providers(&self) -> BTreeSet<String> {
        let mut providers = self.unavailable.clone();
        providers.extend(self.demotable.keys().cloned());
        providers
    }

    fn demotable_providers(&self) -> BTreeMap<String, RuntimeVerdict> {
        self.demotable.clone()
    }

    fn demotion_applied(&self, workspace_id: &str, providers: &BTreeSet<String>) {
        self.launches.remove_builtins(workspace_id, providers);
    }

    fn registration_applied(&self, workspace_id: &str, accepted: &[crate::types::Runtime]) {
        let accepted_builtins: BTreeSet<String> = accepted
            .iter()
            .filter(|runtime| runtime.profile_id.is_empty())
            .map(|runtime| runtime.provider.clone())
            .collect();
        self.launches
            .apply_builtins(workspace_id, &self.builtin_launches, &accepted_builtins);
        let Some(specs) = self.pending_profiles.lock().unwrap().remove(workspace_id) else {
            return;
        };
        let accepted_profiles: BTreeSet<String> = accepted
            .iter()
            .filter(|runtime| !runtime.profile_id.is_empty())
            .map(|runtime| runtime.profile_id.clone())
            .collect();
        self.launches.replace_workspace_profiles(
            workspace_id,
            specs
                .into_iter()
                .filter(|spec| accepted_profiles.contains(&spec.target.profile_id))
                .collect(),
        );
    }
}

#[async_trait::async_trait]
impl<C: ProviderCatalog> RuntimeRegistrationSource for ProviderRegistrationSource<C> {
    async fn begin_round(
        &self,
        ctx: Ctx,
        sampled_after_demotion_seq: u64,
    ) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>> {
        let snapshot = self
            .probe(
                ctx,
                ProviderProbeReason::Registration,
                sampled_after_demotion_seq,
            )
            .await?;
        Ok(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
            demotable: snapshot.demotable,
            unavailable: snapshot.unavailable,
            recovered: snapshot.recovered,
            sampled_after_demotion_seq: snapshot.sampled_after_demotion_seq,
            include_profiles: true,
            pending_profiles: Mutex::new(HashMap::new()),
        }))
    }

    async fn begin_builtin_refresh(
        &self,
        ctx: Ctx,
        reason: BuiltinRefreshReason,
        sampled_after_demotion_seq: u64,
    ) -> anyhow::Result<Option<Arc<dyn RuntimeRegistrationRound>>> {
        let snapshot = self
            .probe(ctx, reason.into(), sampled_after_demotion_seq)
            .await?;
        Ok(Some(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
            demotable: snapshot.demotable,
            unavailable: snapshot.unavailable,
            recovered: snapshot.recovered,
            sampled_after_demotion_seq: snapshot.sampled_after_demotion_seq,
            include_profiles: false,
            pending_profiles: Mutex::new(HashMap::new()),
        })))
    }

    fn workspace_removed(&self, workspace_id: &str) {
        self.launches.remove_workspace(workspace_id);
    }

    fn agent_health_snapshot(&self) -> AgentHealthSnapshot {
        self.agent_health.read().unwrap().clone()
    }
}

fn agent_health_snapshot_from_probe(probe: &ProviderProbeResult) -> AgentHealthSnapshot {
    let mut agents: BTreeSet<String> = probe
        .detected
        .iter()
        .map(|runtime| runtime.provider.clone())
        .collect();
    let mut skipped_agents: HashMap<String, String> = probe.skipped.clone().into_iter().collect();
    skipped_agents.extend(probe.unavailable.clone());
    for (provider, verdict) in &probe.demotable {
        skipped_agents.insert(provider.clone(), verdict.reason.clone());
    }
    agents.extend(skipped_agents.keys().cloned());
    AgentHealthSnapshot {
        agents: agents.into_iter().collect(),
        skipped_agents,
    }
}

fn display_name(name: &str, device_name: &str) -> String {
    if device_name.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({device_name})")
    }
}

fn registration_entry(
    name: String,
    provider: String,
    version: String,
    profile_id: Option<String>,
) -> BTreeMap<String, String> {
    let mut entry = BTreeMap::from([
        ("name".to_string(), name),
        ("status".to_string(), "online".to_string()),
        ("type".to_string(), provider),
        ("version".to_string(), version),
    ]);
    if let Some(profile_id) = profile_id {
        entry.insert("profile_id".to_string(), profile_id);
    }
    entry
}

fn profile_failure(profile: &RuntimeProfile, reason: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("command_name".to_string(), profile.command_name.clone()),
        ("profile_id".to_string(), profile.id.clone()),
        ("reason".to_string(), reason.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct ProbeSequence {
        probes: Mutex<VecDeque<anyhow::Result<ProviderProbeResult>>>,
    }

    #[async_trait::async_trait]
    impl ProviderCatalog for ProbeSequence {
        async fn probe_builtins(
            &self,
            _ctx: Ctx,
            _reason: ProviderProbeReason,
        ) -> anyhow::Result<ProviderProbeResult> {
            self.probes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("no probe result queued")))
        }

        async fn resolve_profile(
            &self,
            _ctx: Ctx,
            _profile: &RuntimeProfile,
            _command_override: Option<&str>,
        ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
            unreachable!("profile resolution is not used by this test")
        }
    }

    #[tokio::test]
    async fn successful_probe_replaces_dynamic_health_diagnostics() {
        let first = ProviderProbeResult {
            detected: vec![DetectedProviderRuntime {
                provider: "claude".to_string(),
                display_name: "Claude".to_string(),
                version: "1.0.0".to_string(),
                command_path: "/bin/claude".to_string(),
                fixed_args: Vec::new(),
            }],
            demotable: BTreeMap::from([(
                "kiro".to_string(),
                RuntimeVerdict {
                    reason: "below minimum".to_string(),
                    offline: None,
                },
            )]),
            unavailable: BTreeMap::from([(
                "codex".to_string(),
                "version probe timed out".to_string(),
            )]),
            skipped: BTreeMap::from([(
                "unsupported".to_string(),
                "provider CLI has no Rust backend".to_string(),
            )]),
        };
        let second = ProviderProbeResult {
            detected: vec![DetectedProviderRuntime {
                provider: "codex".to_string(),
                display_name: "Codex".to_string(),
                version: "2.0.0".to_string(),
                command_path: "/bin/codex".to_string(),
                fixed_args: Vec::new(),
            }],
            ..ProviderProbeResult::default()
        };
        let catalog = Arc::new(ProbeSequence {
            probes: Mutex::new(VecDeque::from([
                Ok(first),
                Ok(second),
                Err(anyhow::anyhow!("discovery unavailable")),
            ])),
        });
        let mut config = Config::default();
        config
            .agents
            .insert("startup".to_string(), crate::types::AgentEntry::default());
        let source = ProviderRegistrationSource::new(
            Arc::new(config),
            Arc::new(Client::new("http://localhost")),
            catalog,
            Arc::new(RuntimeLaunchRegistry::default()),
        );

        assert_eq!(source.agent_health_snapshot().agents, vec!["startup"]);

        source
            .probe(Ctx::new(), ProviderProbeReason::Discovery, 0)
            .await
            .unwrap();
        let first_health = source.agent_health_snapshot();
        assert_eq!(
            first_health.agents,
            vec!["claude", "codex", "kiro", "unsupported"]
        );
        assert_eq!(
            first_health.skipped_agents,
            HashMap::from([
                ("codex".to_string(), "version probe timed out".to_string()),
                ("kiro".to_string(), "below minimum".to_string()),
                (
                    "unsupported".to_string(),
                    "provider CLI has no Rust backend".to_string(),
                ),
            ])
        );

        source
            .probe(Ctx::new(), ProviderProbeReason::Version, 1)
            .await
            .unwrap();
        let second_health = source.agent_health_snapshot();
        assert_eq!(second_health.agents, vec!["codex"]);
        assert!(second_health.skipped_agents.is_empty());

        assert!(source
            .probe(Ctx::new(), ProviderProbeReason::Discovery, 1)
            .await
            .is_err());
        assert_eq!(source.agent_health_snapshot(), second_health);
    }

    #[test]
    fn custom_profiles_require_a_real_protocol_backend() {
        assert!(LocalProviderCatalog::supports_profile_backend("codex"));
        assert!(LocalProviderCatalog::supports_profile_backend("claude"));
        assert!(!LocalProviderCatalog::supports_profile_backend("omp"));
        assert!(!LocalProviderCatalog::supports_profile_backend("unknown"));
    }

    #[test]
    fn launch_registry_keeps_provider_and_profile_identity_atomic() {
        let registry = RuntimeLaunchRegistry::default();
        registry.replace_builtins(
            "ws-1",
            vec![RuntimeLaunchSpec {
                target: RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: String::new(),
                },
                display_name: "Codex".to_string(),
                command_path: "/bin/codex".to_string(),
                fixed_args: Vec::new(),
                version: "1.0.0".to_string(),
            }],
        );
        registry.replace_workspace_profiles(
            "ws-1",
            vec![RuntimeLaunchSpec {
                target: RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
                display_name: "Wrapped Codex".to_string(),
                command_path: "/opt/wrapper".to_string(),
                fixed_args: vec!["start".to_string()],
                version: "2.0.0".to_string(),
            }],
        );

        let builtin = registry
            .resolve(
                "ws-1",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: String::new(),
                },
            )
            .unwrap();
        assert_eq!(builtin.command_path, "/bin/codex");
        assert!(registry
            .resolve(
                "ws-2",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: String::new(),
                },
            )
            .is_none());

        let profile = registry
            .resolve(
                "ws-1",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
            )
            .unwrap();
        assert_eq!(profile.command_path, "/opt/wrapper");
        assert_eq!(profile.fixed_args, vec!["start"]);
        assert!(registry
            .resolve(
                "ws-2",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
            )
            .is_none());
        assert!(registry
            .resolve(
                "ws-1",
                &RuntimeExecutionTarget {
                    provider: "qwen".to_string(),
                    profile_id: "profile-1".to_string(),
                },
            )
            .is_none());
    }

    #[test]
    fn builtin_replacement_is_scoped_to_the_applied_workspace() {
        let registry = RuntimeLaunchRegistry::default();
        let launch = |command_path: &str| RuntimeLaunchSpec {
            target: RuntimeExecutionTarget {
                provider: "codex".to_string(),
                profile_id: String::new(),
            },
            display_name: "Codex".to_string(),
            command_path: command_path.to_string(),
            fixed_args: Vec::new(),
            version: "1.0.0".to_string(),
        };
        registry.replace_builtins("ws-1", vec![launch("/old/codex")]);
        registry.replace_builtins("ws-2", vec![launch("/old/codex")]);

        registry.replace_builtins("ws-1", vec![launch("/new/codex")]);

        let target = RuntimeExecutionTarget {
            provider: "codex".to_string(),
            profile_id: String::new(),
        };
        assert_eq!(
            registry.resolve("ws-1", &target).unwrap().command_path,
            "/new/codex"
        );
        assert_eq!(
            registry.resolve("ws-2", &target).unwrap().command_path,
            "/old/codex"
        );
    }

    #[test]
    fn builtin_refresh_retry_is_scoped_to_the_workspace_that_applied() {
        let registry = RuntimeLaunchRegistry::default();
        let launch = |version: &str| RuntimeLaunchSpec {
            target: RuntimeExecutionTarget {
                provider: "codex".to_string(),
                profile_id: String::new(),
            },
            display_name: "Codex".to_string(),
            command_path: "/bin/codex".to_string(),
            fixed_args: Vec::new(),
            version: version.to_string(),
        };
        registry.replace_builtins("ws-1", vec![launch("1.0.0")]);
        registry.replace_builtins("ws-2", vec![launch("1.0.0")]);
        let refreshed = vec![launch("2.0.0")];

        assert!(registry.builtin_refresh_needed("ws-1", &refreshed));
        assert!(registry.builtin_refresh_needed("ws-2", &refreshed));
        registry.replace_builtins("ws-1", refreshed.clone());
        assert!(!registry.builtin_refresh_needed("ws-1", &refreshed));
        assert!(registry.builtin_refresh_needed("ws-2", &refreshed));
    }

    #[test]
    fn successful_empty_profile_replace_removes_stale_launches() {
        let registry = RuntimeLaunchRegistry::default();
        registry.replace_workspace_profiles(
            "ws-1",
            vec![RuntimeLaunchSpec {
                target: RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
                display_name: "Wrapped Codex".to_string(),
                command_path: "/opt/wrapper".to_string(),
                fixed_args: Vec::new(),
                version: String::new(),
            }],
        );
        registry.replace_workspace_profiles("ws-1", Vec::new());
        assert!(registry
            .resolve(
                "ws-1",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
            )
            .is_none());
    }

    #[test]
    fn workspace_removal_prevents_stale_profile_revival() {
        let registry = RuntimeLaunchRegistry::default();
        registry.replace_workspace_profiles(
            "ws-1",
            vec![RuntimeLaunchSpec {
                target: RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: "profile-1".to_string(),
                },
                display_name: "Wrapped Codex".to_string(),
                command_path: "/opt/wrapper".to_string(),
                fixed_args: Vec::new(),
                version: String::new(),
            }],
        );
        registry.remove_workspace("ws-1");
        assert!(registry.workspace_profiles("ws-1").is_empty());
    }

    #[test]
    fn registration_entry_matches_daemon_wire_shape() {
        assert_eq!(
            registration_entry(
                "Codex (laptop)".to_string(),
                "codex".to_string(),
                "1.2.3".to_string(),
                Some("profile-1".to_string()),
            ),
            BTreeMap::from([
                ("name".to_string(), "Codex (laptop)".to_string()),
                ("profile_id".to_string(), "profile-1".to_string()),
                ("status".to_string(), "online".to_string()),
                ("type".to_string(), "codex".to_string()),
                ("version".to_string(), "1.2.3".to_string()),
            ])
        );
    }

    #[test]
    fn not_executable_requires_two_sightings_even_with_zero_window() {
        let catalog =
            LocalProviderCatalog::with_probe_windows(Duration::from_secs(1), Duration::ZERO);
        let now = Instant::now();
        assert!(!catalog.confirm_not_executable("codex", now));
        assert!(catalog.confirm_not_executable("codex", now));
        catalog.clear_not_executable("codex");
        assert!(!catalog.confirm_not_executable("codex", now));
    }

    #[test]
    fn npm_bin_repair_uses_manifest_postinstall() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("node_modules").join("agent-cli");
        let bin = package.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"agent-cli","scripts":{"postinstall":"node install.cjs"}}"#,
        )
        .unwrap();
        let repair = exec_format_repair(bin.join("agent").to_str().unwrap()).unwrap();
        assert_eq!(repair.package, "agent-cli");
        assert!(repair.command.contains("node install.cjs"));
        #[cfg(windows)]
        assert_eq!(repair.shell, "powershell");
        #[cfg(not(windows))]
        assert_eq!(repair.shell, "bash");
    }

    #[cfg(not(windows))]
    #[test]
    fn exec_format_detection_reads_wrapped_os_error() {
        let error = anyhow::Error::new(io::Error::from_raw_os_error(8)).context("start process");
        assert!(is_exec_format_error(&error));
        assert!(!is_exec_format_error(&anyhow::anyhow!("timeout")));
    }
}
