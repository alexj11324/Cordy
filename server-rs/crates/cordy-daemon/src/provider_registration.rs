//! Provider-backed runtime registration and launch identity publication.
//!
//! The daemon owns workspace/profile orchestration while the provider crate
//! owns executable discovery, version policy, and fixed-argument filtering.
//! This module joins those responsibilities without providing a fallback
//! catalog: production construction requires a real [`ProviderCatalog`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::client::{Client, RuntimeProfile};
use crate::config::Config;
use crate::registration::{
    BuiltinRefreshReason, RegistrationPayload, RuntimeRegistrationRound, RuntimeRegistrationSource,
};
use crate::repocache::Ctx;
use crate::types::RuntimeExecutionTarget;
use cordy_agent::{
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
/// returning an empty successful probe is authoritative and removes vanished
/// built-in runtimes during a refresh.
#[async_trait::async_trait]
pub trait ProviderCatalog: Send + Sync + 'static {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<Vec<DetectedProviderRuntime>>;

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

    fn supports_profile_backend(runtime_id: &str) -> bool {
        // Built-in runtime identities such as `omp` may map to a provider
        // family for execution, but they are not valid custom profile
        // protocol_family values. Keep profile registration on the explicit
        // provider whitelist and fail closed for metadata-only families.
        cordy_agent::provider(runtime_id).is_some() && Self::supports_backend(runtime_id)
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

    async fn resolve_command(
        &self,
        ctx: &Ctx,
        profile: &RuntimeProfile,
        command_override: Option<&str>,
    ) -> Result<ResolvedProfileCommand, ProfileResolutionError> {
        if !Self::supports_profile_backend(&profile.protocol_family) {
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

        let fixed_args =
            filter_launch_prefix_for_provider(&profile.protocol_family, &profile.fixed_args);
        let version = self
            .probe_version(ctx, &command_path, &fixed_args)
            .await
            .map_err(|error| ProfileResolutionError {
                reason: format!("provider version probe failed: {error}"),
            })?;
        Ok(ResolvedProfileCommand {
            command_path,
            fixed_args,
            version,
        })
    }
}

#[async_trait::async_trait]
impl ProviderCatalog for LocalProviderCatalog {
    async fn probe_builtins(
        &self,
        ctx: Ctx,
        _reason: ProviderProbeReason,
    ) -> anyhow::Result<Vec<DetectedProviderRuntime>> {
        let agents = crate::agents_probe::probe_agent_clis();
        let mut detected = Vec::new();
        for (provider, entry) in agents {
            if !Self::supports_backend(&provider) {
                tracing::debug!(%provider, "provider CLI discovered without a Rust backend; withholding registration");
                continue;
            }
            let Some(display_name) = Self::display_name(&provider) else {
                tracing::warn!(%provider, "provider CLI discovered without catalog metadata; withholding registration");
                continue;
            };
            let fixed_args = Self::fixed_args(&provider);
            let version = match self
                .probe_version(&ctx.child(), &entry.path, &fixed_args)
                .await
            {
                Ok(version) => version,
                Err(error) => {
                    tracing::debug!(%provider, error = %error, "provider version probe failed; withholding registration");
                    continue;
                }
            };
            if let Err(error) = check_provider_minimum(&provider, &version) {
                tracing::warn!(%provider, %version, error = %error, "provider CLI is below its minimum version; withholding registration");
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
        Ok(detected)
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
    last_builtin_payload: Mutex<Option<Vec<BTreeMap<String, String>>>>,
}

impl<C: ProviderCatalog> ProviderRegistrationSource<C> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        catalog: Arc<C>,
        launches: Arc<RuntimeLaunchRegistry>,
    ) -> Self {
        Self {
            config,
            client,
            catalog,
            launches,
            last_builtin_payload: Mutex::new(None),
        }
    }

    async fn probe(
        &self,
        ctx: Ctx,
        reason: ProviderProbeReason,
    ) -> anyhow::Result<BuiltinSnapshot> {
        let mut detected = self.catalog.probe_builtins(ctx, reason).await?;
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
        Ok(BuiltinSnapshot { payload, launches })
    }
}

struct BuiltinSnapshot {
    payload: Vec<BTreeMap<String, String>>,
    launches: Vec<RuntimeLaunchSpec>,
}

struct ProviderRegistrationRound<C: ProviderCatalog> {
    config: Arc<Config>,
    client: Arc<Client>,
    catalog: Arc<C>,
    launches: Arc<RuntimeLaunchRegistry>,
    builtins: Vec<BTreeMap<String, String>>,
    builtin_launches: Vec<RuntimeLaunchSpec>,
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
        self.launches
            .replace_builtins(workspace_id, self.builtin_launches.clone());
        let Some(specs) = self.pending_profiles.lock().unwrap().remove(workspace_id) else {
            return;
        };
        self.launches
            .replace_workspace_profiles(workspace_id, specs);
    }
}

#[async_trait::async_trait]
impl<C: ProviderCatalog> RuntimeRegistrationSource for ProviderRegistrationSource<C> {
    async fn begin_round(&self, ctx: Ctx) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>> {
        let snapshot = self.probe(ctx, ProviderProbeReason::Registration).await?;
        *self.last_builtin_payload.lock().unwrap() = Some(snapshot.payload.clone());
        Ok(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
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
        let changed = {
            let mut previous = self.last_builtin_payload.lock().unwrap();
            if previous.as_ref() == Some(&snapshot.payload) {
                false
            } else {
                *previous = Some(snapshot.payload.clone());
                true
            }
        };
        if !changed {
            return Ok(None);
        }
        Ok(Some(Arc::new(ProviderRegistrationRound {
            config: Arc::clone(&self.config),
            client: Arc::clone(&self.client),
            catalog: Arc::clone(&self.catalog),
            launches: Arc::clone(&self.launches),
            builtins: snapshot.payload,
            builtin_launches: snapshot.launches,
            include_profiles: false,
            pending_profiles: Mutex::new(HashMap::new()),
        })))
    }

    fn workspace_removed(&self, workspace_id: &str) {
        self.launches.remove_workspace(workspace_id);
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

    #[test]
    fn custom_profiles_require_a_real_protocol_backend() {
        assert!(LocalProviderCatalog::supports_profile_backend("codex"));
        assert!(!LocalProviderCatalog::supports_profile_backend("omp"));
        assert!(!LocalProviderCatalog::supports_profile_backend("claude"));
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
