//! Provider-backed runtime registration and launch identity publication.
//!
//! The daemon owns workspace/profile orchestration while the provider crate
//! owns executable discovery, version policy, and fixed-argument filtering.
//! This module joins those responsibilities without providing a fallback
//! catalog: production construction requires a real [`ProviderCatalog`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::agents_refresh::RuntimeVerdict;
use crate::client::{
    Client, Repair, RuntimeOfflineReason, RuntimeProfile, RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE,
};
use crate::config::Config;
use crate::registration::{
    BuiltinRefreshReason, RegistrationPayload, RuntimeRegistrationRound, RuntimeRegistrationSource,
};
use crate::repocache::Ctx;
use crate::types::{AgentEntry, RuntimeExecutionTarget};
use cordy_agent::{build_backend, check_provider_minimum, extract_version_line};

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

/// One machine-level built-in probe round. Availability is intentionally kept
/// separate from launchable detections: Go's daemon keeps a discovered CLI in
/// its availability set even when version probing or Rust backend capability
/// prevents registration, so health can explain the skipped provider and the
/// next discovery round can retry it.
#[derive(Debug, Clone, Default)]
pub struct BuiltinProbeResult {
    pub available: BTreeMap<String, AgentEntry>,
    pub detected: Vec<DetectedProviderRuntime>,
    pub skipped: BTreeMap<String, String>,
    /// Confirmed version/launch verdicts. These are acted on only by the
    /// periodic version refresh, which owns the claim barrier and demotion
    /// hold needed to take an existing runtime offline safely.
    pub demotable: BTreeMap<String, RuntimeVerdict>,
    /// Raw exec-format findings. The source requires the Go-compatible
    /// confirmation window before moving one into `demotable`.
    pub not_executable: BTreeMap<String, RuntimeVerdict>,
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
}

/// Mandatory provider-core surface. Failures must be returned as errors;
/// returning an empty successful probe is authoritative for a full workspace
/// registration, while built-in refresh preserves the current runtime set.
#[async_trait::async_trait]
pub trait ProviderCatalog: Send + Sync + 'static {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<BuiltinProbeResult>;

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
/// `agents_probe` owns PATH/login-shell discovery and `cordy-agent` owns the
/// provider-family/backend capability table. It never manufactures a
/// metadata-only backend. Unsupported families are omitted from registration
/// until their real `cordy-agent` transport lands, while custom profiles carry
/// a structured failure back to the server.
#[derive(Debug, Clone, Copy)]
pub struct LocalProviderCatalog {
    version_probe_timeout: Duration,
}

impl Default for LocalProviderCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalProviderCatalog {
    const DEFAULT_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
    const VERSION_PROBE_ATTEMPTS: usize = 2;
    const VERSION_PROBE_RETRY_DELAY: Duration = Duration::from_millis(500);
    const VERSION_PROBE_RETRY_WINDOW: Duration = Duration::from_secs(1);

    pub const fn new() -> Self {
        Self {
            version_probe_timeout: Self::DEFAULT_VERSION_PROBE_TIMEOUT,
        }
    }

    /// Test/embedding seam for the bounded `--version` process probe. The
    /// production default remains ten seconds, matching the Go daemon's
    /// per-provider probe budget.
    pub const fn with_version_probe_timeout(timeout: Duration) -> Self {
        Self {
            version_probe_timeout: timeout,
        }
    }

    fn supports_backend(runtime_id: &str) -> bool {
        build_backend(runtime_id, cordy_agent::BackendConfig::default()).is_ok()
    }

    fn display_name(provider: &str) -> Option<&'static str> {
        cordy_agent::provider(provider)
            .map(|descriptor| descriptor.display_name)
            .or_else(|| cordy_agent::builtin_runtime(provider).map(|runtime| runtime.display_name))
    }

    fn fixed_args(provider: &str) -> Vec<String> {
        // DSH is probed with the Cordy profile, and the same profile selector
        // must precede its protocol-owned `--version` argument at launch.
        if provider == "dsh" {
            vec!["--profile".to_string(), "cordy".to_string()]
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
        let output =
            crate::gc::processtree::output(ctx, command, self.version_probe_timeout).await?;
        Ok(extract_version_line(&String::from_utf8_lossy(&output)))
    }

    async fn probe_version_with_retry(
        &self,
        ctx: &Ctx,
        command_path: &str,
        fixed_args: &[String],
    ) -> Result<String, VersionProbeFailure> {
        let mut last_error: Option<(String, bool)> = None;
        for attempt in 0..Self::VERSION_PROBE_ATTEMPTS {
            if attempt > 0 {
                let sleep = tokio::time::sleep(Self::VERSION_PROBE_RETRY_DELAY);
                tokio::pin!(sleep);
                tokio::select! {
                    () = ctx.cancelled() => break,
                    () = &mut sleep => {}
                }
                if ctx.err().is_some() {
                    break;
                }
            }
            let started = Instant::now();
            match self.probe_version(ctx, command_path, fixed_args).await {
                Ok(version) => return Ok(version),
                Err(error) => {
                    let is_exec_format = is_exec_format_error(&error);
                    last_error = Some((error.to_string(), is_exec_format));
                    if attempt + 1 < Self::VERSION_PROBE_ATTEMPTS
                        && started.elapsed() < Self::VERSION_PROBE_RETRY_WINDOW
                    {
                        continue;
                    }
                    break;
                }
            }
        }

        let (reason, is_exec_format) =
            last_error.unwrap_or_else(|| ("provider version probe cancelled".to_string(), false));
        if is_exec_format {
            Err(VersionProbeFailure::NotExecutable(reason))
        } else {
            Err(VersionProbeFailure::Unavailable(reason))
        }
    }

    async fn resolve_command(
        &self,
        ctx: &Ctx,
        profile: &RuntimeProfile,
        command_override: Option<&str>,
    ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
        if !Self::supports_backend(&profile.protocol_family) {
            return Err(ProfileResolutionError {
                reason: format!("unsupported protocol family: {}", profile.protocol_family),
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
            })?;

        let fixed_args = profile.fixed_args.clone();
        let version = self
            .probe_version(ctx, &command_path, &fixed_args)
            .await
            .map_err(|error| ProfileResolutionError {
                reason: format!("provider version probe failed: {error}"),
            })?;
        check_provider_minimum(&profile.protocol_family, &version).map_err(|error| {
            ProfileResolutionError {
                reason: format!("provider version {version:?} is not supported: {error}"),
            }
        })?;
        Ok(ResolvedProfileCommand {
            command_path,
            fixed_args,
            version,
        })
    }
}

#[derive(Debug)]
enum VersionProbeFailure {
    Unavailable(String),
    NotExecutable(String),
}

fn is_exec_format_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .is_some_and(|code| {
                #[cfg(unix)]
                {
                    code == libc::ENOEXEC
                }
                #[cfg(windows)]
                {
                    code == 193
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = code;
                    false
                }
            })
    })
}

fn exec_format_repair_for(command_path: &str) -> Option<Repair> {
    let bin = Path::new(command_path).parent()?;
    let bin_name = bin.file_name()?.to_string_lossy();
    if !bin_name.eq_ignore_ascii_case("bin") {
        return None;
    }
    let root = bin.parent()?;
    let manifest = std::fs::read(root.join("package.json")).ok()?;
    #[derive(serde::Deserialize)]
    struct PackageManifest {
        #[serde(default)]
        name: String,
        #[serde(default)]
        scripts: PackageScripts,
    }
    #[derive(Default, serde::Deserialize)]
    struct PackageScripts {
        #[serde(default)]
        postinstall: String,
    }
    let manifest: PackageManifest = serde_json::from_slice(&manifest).ok()?;
    let script = manifest.scripts.postinstall.trim();
    if script.is_empty() {
        return None;
    }
    let package = if manifest.name.trim().is_empty() {
        root.file_name()?.to_string_lossy().into_owned()
    } else {
        manifest.name.trim().to_string()
    };
    let root = root.to_string_lossy();
    #[cfg(windows)]
    let (shell, command) = (
        "powershell",
        format!("Set-Location {}\n{script}", powershell_quote(&root)),
    );
    #[cfg(not(windows))]
    let (shell, command) = ("bash", format!("cd {} && {script}", shell_quote(&root)));
    Some(Repair {
        package,
        command,
        shell: shell.to_string(),
    })
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[async_trait::async_trait]
impl ProviderCatalog for LocalProviderCatalog {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        _reason: ProviderProbeReason,
    ) -> anyhow::Result<BuiltinProbeResult> {
        let agents = crate::agents_probe::probe_agent_clis();
        let available = agents.clone();
        let mut detected = Vec::new();
        let mut skipped = BTreeMap::new();
        let mut demotable = BTreeMap::new();
        let mut not_executable = BTreeMap::new();
        for (provider, entry) in agents {
            if !Self::supports_backend(&provider) {
                let reason = "provider CLI discovered without a Rust backend";
                tracing::debug!(%provider, reason, "withholding provider registration");
                skipped.insert(provider, reason.to_string());
                continue;
            }
            let Some(display_name) = Self::display_name(&provider) else {
                let reason = "provider CLI discovered without catalog metadata";
                tracing::warn!(%provider, reason, "withholding provider registration");
                skipped.insert(provider, reason.to_string());
                continue;
            };
            let fixed_args = Self::fixed_args(&provider);
            let version = match self
                .probe_version_with_retry(&ctx.child(), &entry.path, &fixed_args)
                .await
            {
                Ok(version) => version,
                Err(VersionProbeFailure::Unavailable(error)) => {
                    let reason = format!("provider version probe failed: {error}");
                    tracing::debug!(%provider, error, "withholding provider registration");
                    skipped.insert(provider, reason);
                    continue;
                }
                Err(VersionProbeFailure::NotExecutable(error)) => {
                    let reason = format!("agent CLI is not executable: {error}");
                    tracing::warn!(%provider, error, "withholding provider registration");
                    skipped.insert(provider.clone(), reason.clone());
                    not_executable.insert(
                        provider,
                        RuntimeVerdict {
                            reason: reason.clone(),
                            offline: Some(RuntimeOfflineReason {
                                code: RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.to_string(),
                                detail: reason,
                                repair: exec_format_repair_for(&entry.path),
                            }),
                        },
                    );
                    continue;
                }
            };
            if let Err(error) = check_provider_minimum(&provider, &version) {
                let reason =
                    format!("provider CLI version {version:?} is below its minimum: {error}");
                tracing::warn!(%provider, %version, %error, "withholding provider registration");
                skipped.insert(provider.clone(), reason.clone());
                if matches!(error, cordy_agent::version::VersionError::TooOld) {
                    demotable.insert(
                        provider,
                        RuntimeVerdict {
                            reason,
                            offline: None,
                        },
                    );
                }
                continue;
            }
            detected.push(DetectedProviderRuntime {
                provider,
                display_name: display_name.to_string(),
                version,
                command_path: entry.path,
                fixed_args,
            });
        }
        Ok(BuiltinProbeResult {
            available,
            detected,
            skipped,
            demotable,
            not_executable,
        })
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

    pub(crate) fn replace_builtins_preserving(
        &self,
        workspace_id: &str,
        specs: Vec<RuntimeLaunchSpec>,
        providers: &BTreeSet<String>,
    ) {
        let mut state = self.state.write().unwrap();
        let mut builtins = specs
            .into_iter()
            .map(|spec| (spec.target.provider.clone(), spec))
            .collect::<BTreeMap<_, _>>();
        if let Some(previous) = state.builtins.get(workspace_id) {
            for provider in providers {
                if let Some(spec) = previous.get(provider) {
                    builtins
                        .entry(provider.clone())
                        .or_insert_with(|| spec.clone());
                }
            }
        }
        state.builtins.insert(workspace_id.to_string(), builtins);
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
    available_agents: Mutex<BTreeMap<String, AgentEntry>>,
    skipped_agents: Mutex<BTreeMap<String, String>>,
    not_executable_since: Mutex<BTreeMap<String, Instant>>,
}

impl<C: ProviderCatalog> ProviderRegistrationSource<C> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        catalog: Arc<C>,
        launches: Arc<RuntimeLaunchRegistry>,
    ) -> Self {
        let available_agents = config.agents.clone();
        Self {
            config,
            client,
            catalog,
            launches,
            available_agents: Mutex::new(available_agents),
            skipped_agents: Mutex::new(BTreeMap::new()),
            not_executable_since: Mutex::new(BTreeMap::new()),
        }
    }

    /// Health-facing view of the machine discovery state. Availability is
    /// copy-on-write and one-directional, matching Go's `agentsAvailable`:
    /// transient PATH/version-probe loss must not make a running provider
    /// disappear from the diagnostic set.
    pub fn health_snapshot(&self) -> (Vec<String>, HashMap<String, String>) {
        let agents = self
            .available_agents
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let skipped = self
            .skipped_agents
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .collect();
        (agents, skipped)
    }

    async fn probe(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<BuiltinSnapshot> {
        let mut probe = self.catalog.probe_builtins(ctx, reason).await?;
        let gained = {
            let mut current = self.available_agents.lock().unwrap();
            if let Some(merged) =
                crate::agents_refresh::merge_discovered_agents(&current, &probe.available)
            {
                *current = merged;
                true
            } else {
                false
            }
        };

        // Only a provider that completed a supported version probe is healthy
        // enough to clear an old exec-format sighting. `available` also
        // contains CLIs whose probe failed, so using it here would reset the
        // confirmation clock on every broken executable and make demotion
        // unreachable.
        let recovered = probe
            .detected
            .iter()
            .map(|runtime| runtime.provider.clone())
            .collect::<Vec<_>>();
        let now = Instant::now();
        {
            let mut pending = self.not_executable_since.lock().unwrap();
            for provider in &recovered {
                pending.remove(provider);
            }
            let not_executable = probe.not_executable.keys().cloned().collect::<Vec<_>>();
            for provider in not_executable {
                let first = pending.entry(provider.clone()).or_insert(now);
                if now.duration_since(*first) >= NOT_EXECUTABLE_CONFIRM_WINDOW {
                    if let Some(verdict) = probe.not_executable.remove(&provider) {
                        probe.demotable.insert(provider, verdict);
                    }
                }
            }
        }

        let preserve_providers = probe.skipped.keys().cloned().collect::<BTreeSet<_>>();
        *self.skipped_agents.lock().unwrap() = probe.skipped;

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
        Ok(BuiltinSnapshot {
            payload,
            launches,
            gained,
            demotable: probe.demotable,
            recovered,
            preserve_providers,
        })
    }

    #[cfg(test)]
    fn age_not_executable_for_test(&self, provider: &str) {
        let mut pending = self.not_executable_since.lock().unwrap();
        pending.insert(
            provider.to_string(),
            Instant::now() - NOT_EXECUTABLE_CONFIRM_WINDOW - Duration::from_secs(1),
        );
    }
}

const NOT_EXECUTABLE_CONFIRM_WINDOW: Duration = Duration::from_secs(60);

struct BuiltinSnapshot {
    payload: Vec<BTreeMap<String, String>>,
    launches: Vec<RuntimeLaunchSpec>,
    gained: bool,
    demotable: BTreeMap<String, RuntimeVerdict>,
    recovered: Vec<String>,
    preserve_providers: BTreeSet<String>,
}

struct ProviderRegistrationRound<C: ProviderCatalog> {
    config: Arc<Config>,
    client: Arc<Client>,
    catalog: Arc<C>,
    launches: Arc<RuntimeLaunchRegistry>,
    builtins: Vec<BTreeMap<String, String>>,
    builtin_launches: Vec<RuntimeLaunchSpec>,
    gained: bool,
    demotable: BTreeMap<String, RuntimeVerdict>,
    recovered: Vec<String>,
    preserve_providers: BTreeSet<String>,
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
                for spec in self.launches.workspace_profiles(workspace_id) {
                    payload.runtimes.push(registration_entry(
                        spec.display_name,
                        spec.target.provider,
                        spec.version,
                        Some(spec.target.profile_id),
                    ));
                }
                return Ok(payload);
            }
        };
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
                        fixed_args: command.fixed_args,
                        version: command.version,
                    });
                }
                Err(error) => payload
                    .failed_profiles
                    .push(profile_failure(&profile, &error.reason)),
            }
        }
        self.pending_profiles
            .lock()
            .unwrap()
            .insert(workspace_id.to_string(), launches);
        Ok(payload)
    }

    fn registration_applied(&self, workspace_id: &str) {
        self.launches.replace_builtins_preserving(
            workspace_id,
            self.builtin_launches.clone(),
            &self.preserve_providers,
        );
        let Some(specs) = self.pending_profiles.lock().unwrap().remove(workspace_id) else {
            return;
        };
        self.launches
            .replace_workspace_profiles(workspace_id, specs);
    }

    fn gained_providers(&self) -> bool {
        self.gained
    }

    fn demotable_providers(&self) -> BTreeMap<String, RuntimeVerdict> {
        self.demotable.clone()
    }

    fn recovered_providers(&self) -> Vec<String> {
        self.recovered.clone()
    }

    fn preserve_providers(&self) -> BTreeSet<String> {
        self.preserve_providers.clone()
    }
}

#[async_trait::async_trait]
impl<C: ProviderCatalog> RuntimeRegistrationSource for ProviderRegistrationSource<C> {
    async fn begin_round(&self, ctx: Ctx) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>> {
        let snapshot = self.probe(ctx, ProviderProbeReason::Registration).await?;
        Ok(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
            gained: snapshot.gained,
            demotable: snapshot.demotable,
            recovered: snapshot.recovered,
            preserve_providers: snapshot.preserve_providers,
            include_profiles: true,
            pending_profiles: Mutex::new(HashMap::new()),
        }))
    }

    async fn begin_builtin_refresh(
        &self,
        ctx: Ctx,
        reason: BuiltinRefreshReason,
    ) -> anyhow::Result<Option<Arc<dyn RuntimeRegistrationRound>>> {
        let snapshot = self.probe(ctx, reason.into()).await?;
        Ok(Some(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
            gained: snapshot.gained,
            demotable: snapshot.demotable,
            recovered: snapshot.recovered,
            preserve_providers: snapshot.preserve_providers,
            include_profiles: false,
            pending_profiles: Mutex::new(HashMap::new()),
        })))
    }

    fn workspace_removed(&self, workspace_id: &str) {
        self.launches.remove_workspace(workspace_id);
    }

    fn health_snapshot(&self) -> Option<(Vec<String>, HashMap<String, String>)> {
        Some(Self::health_snapshot(self))
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
    use super::*;

    #[derive(Default)]
    struct FakeCatalog {
        probes: Mutex<Vec<BuiltinProbeResult>>,
    }

    #[async_trait::async_trait]
    impl ProviderCatalog for FakeCatalog {
        async fn probe_builtins(
            &self,
            _ctx: Ctx,
            _reason: ProviderProbeReason,
        ) -> anyhow::Result<BuiltinProbeResult> {
            let mut probes = self.probes.lock().unwrap();
            if probes.is_empty() {
                Err(anyhow::anyhow!("no fake probe result"))
            } else {
                Ok(probes.remove(0))
            }
        }

        async fn resolve_profile(
            &self,
            _ctx: Ctx,
            _profile: &RuntimeProfile,
            _command_override: Option<&str>,
        ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
            Err(ProfileResolutionError {
                reason: "not used in provider refresh test".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn refresh_source_keeps_health_state_and_does_not_cache_away_rounds() {
        let catalog = Arc::new(FakeCatalog {
            // remove(0) consumes in probe order: the first round discovers the
            // provider, while the second reports it without a gain.
            probes: Mutex::new(vec![
                BuiltinProbeResult {
                    available: BTreeMap::from([(
                        "claude".to_string(),
                        AgentEntry {
                            path: "/bin/claude".to_string(),
                            ..AgentEntry::default()
                        },
                    )]),
                    detected: Vec::new(),
                    skipped: BTreeMap::new(),
                    ..BuiltinProbeResult::default()
                },
                BuiltinProbeResult {
                    available: BTreeMap::from([(
                        "claude".to_string(),
                        AgentEntry {
                            path: "/bin/claude".to_string(),
                            ..AgentEntry::default()
                        },
                    )]),
                    detected: Vec::new(),
                    skipped: BTreeMap::from([(
                        "claude".to_string(),
                        "provider version probe failed".to_string(),
                    )]),
                    ..BuiltinProbeResult::default()
                },
            ]),
        });
        let source = ProviderRegistrationSource::new(
            Arc::new(Config::default()),
            Arc::new(Client::new("http://127.0.0.1:1")),
            catalog,
            Arc::new(RuntimeLaunchRegistry::default()),
        );

        let first = source
            .begin_builtin_refresh(Ctx::new(), BuiltinRefreshReason::Discovery)
            .await
            .unwrap()
            .unwrap();
        assert!(first.gained_providers());
        let (agents, skipped) = source.health_snapshot();
        assert_eq!(agents, vec!["claude".to_string()]);
        assert!(skipped.is_empty());

        let second = source
            .begin_builtin_refresh(Ctx::new(), BuiltinRefreshReason::Discovery)
            .await
            .unwrap()
            .unwrap();
        assert!(!second.gained_providers());
        let (agents, skipped) = source.health_snapshot();
        assert_eq!(agents, vec!["claude".to_string()]);
        assert_eq!(
            skipped.get("claude").map(String::as_str),
            Some("provider version probe failed")
        );
    }

    #[tokio::test]
    async fn not_executable_probe_requires_confirmation_before_demotion() {
        let catalog = Arc::new(FakeCatalog {
            probes: Mutex::new(vec![
                BuiltinProbeResult {
                    available: BTreeMap::from([(
                        "claude".to_string(),
                        AgentEntry {
                            path: "/bin/claude".to_string(),
                            ..AgentEntry::default()
                        },
                    )]),
                    skipped: BTreeMap::from([(
                        "claude".to_string(),
                        "agent CLI is not executable".to_string(),
                    )]),
                    not_executable: BTreeMap::from([(
                        "claude".to_string(),
                        RuntimeVerdict {
                            reason: "agent CLI is not executable".to_string(),
                            offline: Some(RuntimeOfflineReason {
                                code: RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.to_string(),
                                detail: "exec format error".to_string(),
                                repair: None,
                            }),
                        },
                    )]),
                    ..BuiltinProbeResult::default()
                },
                BuiltinProbeResult {
                    available: BTreeMap::from([(
                        "claude".to_string(),
                        AgentEntry {
                            path: "/bin/claude".to_string(),
                            ..AgentEntry::default()
                        },
                    )]),
                    skipped: BTreeMap::from([(
                        "claude".to_string(),
                        "agent CLI is not executable".to_string(),
                    )]),
                    not_executable: BTreeMap::from([(
                        "claude".to_string(),
                        RuntimeVerdict {
                            reason: "agent CLI is not executable".to_string(),
                            offline: Some(RuntimeOfflineReason {
                                code: RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.to_string(),
                                detail: "exec format error".to_string(),
                                repair: None,
                            }),
                        },
                    )]),
                    ..BuiltinProbeResult::default()
                },
            ]),
        });
        let source = ProviderRegistrationSource::new(
            Arc::new(Config::default()),
            Arc::new(Client::new("http://127.0.0.1:1")),
            catalog,
            Arc::new(RuntimeLaunchRegistry::default()),
        );

        let first = source
            .begin_builtin_refresh(Ctx::new(), BuiltinRefreshReason::Version)
            .await
            .unwrap()
            .unwrap();
        assert!(first.demotable_providers().is_empty());
        assert!(first.preserve_providers().contains("claude"));

        // The second probe is deliberately aged past the window. This keeps
        // the test fast while still proving that `available` alone cannot
        // clear the pending not-executable sighting.
        source.age_not_executable_for_test("claude");
        let second = source
            .begin_builtin_refresh(Ctx::new(), BuiltinRefreshReason::Version)
            .await
            .unwrap()
            .unwrap();
        let verdict = second.demotable_providers().remove("claude");
        assert!(verdict.is_some(), "confirmed probe must be demotable");
        assert_eq!(
            verdict.unwrap().offline.unwrap().code,
            RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE
        );
    }

    #[test]
    fn exec_format_repair_uses_packaged_cli_postinstall() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("claude-code");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(
            root.join("package.json"),
            br#"{"name":"@anthropic-ai/claude-code","scripts":{"postinstall":"node install.cjs"}}"#,
        )
        .unwrap();
        let command_path = root.join("bin").join("claude");
        let repair = exec_format_repair_for(command_path.to_str().unwrap()).unwrap();

        assert_eq!(repair.package, "@anthropic-ai/claude-code");
        assert_eq!(repair.shell, "bash");
        assert!(repair.command.contains("cd "));
        assert!(repair.command.contains("node install.cjs"));
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
    fn skipped_builtin_replacement_preserves_previous_launch_spec() {
        let registry = RuntimeLaunchRegistry::default();
        let launch = |provider: &str, command_path: &str| RuntimeLaunchSpec {
            target: RuntimeExecutionTarget {
                provider: provider.to_string(),
                profile_id: String::new(),
            },
            display_name: provider.to_string(),
            command_path: command_path.to_string(),
            fixed_args: Vec::new(),
            version: "1.0.0".to_string(),
        };
        registry.replace_builtins(
            "ws-1",
            vec![
                launch("codex", "/bin/codex"),
                launch("claude", "/bin/claude"),
            ],
        );
        registry.replace_builtins_preserving(
            "ws-1",
            vec![launch("codex", "/new/codex")],
            &BTreeSet::from(["claude".to_string()]),
        );

        let target = RuntimeExecutionTarget {
            provider: "claude".to_string(),
            profile_id: String::new(),
        };
        assert_eq!(
            registry.resolve("ws-1", &target).unwrap().command_path,
            "/bin/claude"
        );
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
}
