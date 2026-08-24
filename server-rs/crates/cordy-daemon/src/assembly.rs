//! Production input assembly for the daemon binary.
//!
//! CLI profile parsing stays owned by `cordy-cli`; this module accepts the
//! resolved, non-secret values and turns them into the daemon's authoritative
//! [`Config`], authenticated [`Client`], and repository [`Cache`]. Keeping the
//! boundary here prevents a binary from reimplementing configuration
//! precedence or accidentally starting the stack before authentication and
//! workspace-root safety have been established.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use crate::bootstrap::{self, BootstrapContext, BootstrapOptions, BootstrapOutcome};
use crate::client::Client;
use crate::config::{self, CliProfileConfig, Config, Overrides};
use crate::execenv::context::ensure_workspaces_root_marker;
use crate::health::RepoCheckoutRegistry;
use crate::production_services::{DaemonProductionServices, ProviderRuntimeAdapter};
use crate::production_stack::DaemonProductionStack;
use crate::provider_adapter::ProductionProviderAdapter;
use crate::provider_registration::{
    ProviderCatalog, ProviderRegistrationSource, RuntimeLaunchRegistry,
};
use crate::registration::RuntimeRegistrationSource;
use crate::repocache::Cache;
use crate::types::AgentEntry;

/// Resolved daemon flags and persisted settings supplied by the CLI layer.
/// Environment/default precedence remains implemented by `config::load_config`.
#[derive(Debug, Clone, Default)]
pub struct DaemonLaunchOverrides {
    pub server_url: String,
    pub workspaces_root: String,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub agent_timeout: Option<Duration>,
    pub codex_semantic_inactivity_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    pub max_concurrent_tasks: i64,
    pub daemon_id: String,
    pub device_name: String,
    pub runtime_name: String,
    pub profile: String,
    pub health_port: i32,
    pub allow_no_agents: bool,
    pub disable_auto_update: bool,
    pub auto_update_check_interval: Duration,
    pub disable_auto_reload: bool,
}

impl DaemonLaunchOverrides {
    /// Constructs the one canonical foreground child/successor invocation.
    /// Authentication stays in profile storage and is never copied to argv.
    pub fn foreground_args(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("daemon"),
            OsString::from("start"),
            OsString::from("--foreground"),
        ];
        push_string_arg(&mut args, "--daemon-id", &self.daemon_id);
        push_string_arg(&mut args, "--device-name", &self.device_name);
        push_string_arg(&mut args, "--runtime-name", &self.runtime_name);
        push_string_arg(&mut args, "--workspaces-root", &self.workspaces_root);
        push_duration_arg(&mut args, "--poll-interval", self.poll_interval, false);
        push_duration_arg(
            &mut args,
            "--heartbeat-interval",
            self.heartbeat_interval,
            false,
        );
        if let Some(timeout) = self.agent_timeout {
            push_duration_arg(&mut args, "--agent-timeout", timeout, true);
        }
        push_duration_arg(
            &mut args,
            "--codex-semantic-inactivity-timeout",
            self.codex_semantic_inactivity_timeout,
            false,
        );
        push_duration_arg(
            &mut args,
            "--codex-handshake-timeout",
            self.codex_handshake_timeout,
            false,
        );
        if self.max_concurrent_tasks > 0 {
            push_string_arg(
                &mut args,
                "--max-concurrent-tasks",
                &self.max_concurrent_tasks.to_string(),
            );
        }
        if self.health_port > 0 {
            push_string_arg(&mut args, "--health-port", &self.health_port.to_string());
        }
        if self.disable_auto_update {
            args.push(OsString::from("--no-auto-update"));
        }
        push_duration_arg(
            &mut args,
            "--auto-update-interval",
            self.auto_update_check_interval,
            false,
        );
        if self.disable_auto_reload {
            args.push(OsString::from("--no-auto-reload"));
        }
        push_string_arg(&mut args, "--server-url", &self.server_url);
        push_string_arg(&mut args, "--profile", &self.profile);
        args
    }
}

fn push_string_arg(args: &mut Vec<OsString>, flag: &str, value: &str) {
    if !value.is_empty() {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
}

fn push_duration_arg(args: &mut Vec<OsString>, flag: &str, value: Duration, include_zero: bool) {
    if include_zero || !value.is_zero() {
        args.push(OsString::from(flag));
        args.push(OsString::from(if value.is_zero() {
            "0s".to_string()
        } else {
            format!("{value:?}")
        }));
    }
}

/// Profile data read by `cordy-cli` and consumed during daemon assembly.
///
/// This type intentionally does not implement `Debug`: the bearer token must
/// never be emitted by derived diagnostics or structured startup logs.
#[derive(Clone, Default)]
pub struct DaemonProfileInput {
    pub token: String,
    pub profile_command_overrides: BTreeMap<String, String>,
    pub openclaw_binary_path: String,
    pub openclaw_state_dir: String,
    pub openclaw_cli_timeout: String,
}

/// Fully initialized daemon-owned inputs. Constructing this value proves that
/// configuration, authentication, the root marker, and cache location have
/// all passed their production checks.
pub struct DaemonProductionInputs {
    pub config: Arc<Config>,
    pub client: Arc<Client>,
    pub repo_cache: Arc<Cache>,
    /// Shared by the registration source and provider adapter. A provider
    /// must resolve launches from the same accepted registration state that
    /// the daemon publishes; creating a second registry would allow an
    /// execution path to observe stale or unrelated commands.
    pub launch_registry: Arc<RuntimeLaunchRegistry>,
}

/// Complete set of real services returned by the CLI-side profile/provider
/// loader after bootstrap has established process ownership and logging.
pub struct DaemonProductionAssembly<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> {
    pub inputs: DaemonProductionInputs,
    pub provider: Arc<P>,
    pub registration_source: Arc<R>,
    pub checkout_registry: Arc<RepoCheckoutRegistry>,
}

/// Process-level production entrypoint for the Rust CLI daemon command.
///
/// `build` runs after PID ownership, log installation, and signal setup. It
/// must load the active CLI profile and construct a real provider adapter; the
/// returned stack then owns all background services until bounded shutdown and
/// optional successor handoff complete.
pub async fn run_production_daemon<P, R, Build>(
    options: BootstrapOptions,
    build: Build,
) -> anyhow::Result<BootstrapOutcome>
where
    P: ProviderRuntimeAdapter,
    R: RuntimeRegistrationSource,
    Build: FnOnce(&BootstrapContext) -> anyhow::Result<DaemonProductionAssembly<P, R>>,
{
    bootstrap::run_once(options, move |context| async move {
        let assembly = build(&context)?;
        let stack = assembly
            .inputs
            .into_stack(
                assembly.provider,
                assembly.registration_source,
                assembly.checkout_registry,
            )
            .await?;
        stack.run(context.shutdown).await
    })
    .await
}

impl DaemonProductionInputs {
    /// Production constructor used after the CLI has loaded the active
    /// profile. Agent discovery is real and startup fails when no supported
    /// executable is present unless the explicit probe-only escape hatch is
    /// selected.
    pub fn resolve(
        launch: DaemonLaunchOverrides,
        profile: DaemonProfileInput,
        cli_version: impl Into<String>,
        launched_by: impl Into<String>,
    ) -> anyhow::Result<Self> {
        Self::resolve_with_probe(
            launch,
            profile,
            cli_version.into(),
            launched_by.into(),
            &crate::agents_probe::probe_agent_clis,
        )
    }

    fn resolve_with_probe(
        launch: DaemonLaunchOverrides,
        profile: DaemonProfileInput,
        cli_version: String,
        launched_by: String,
        probe_agents: &dyn Fn() -> BTreeMap<String, AgentEntry>,
    ) -> anyhow::Result<Self> {
        if profile.token.is_empty() {
            let login_hint = if launch.profile.is_empty() {
                "'cordy login'".to_string()
            } else {
                format!("'cordy login --profile {}'", launch.profile)
            };
            anyhow::bail!("not authenticated: run {login_hint} first");
        }

        let overrides = Overrides {
            server_url: launch.server_url,
            workspaces_root: launch.workspaces_root,
            poll_interval: launch.poll_interval,
            heartbeat_interval: launch.heartbeat_interval,
            agent_timeout: launch.agent_timeout,
            codex_semantic_inactivity_timeout: launch.codex_semantic_inactivity_timeout,
            codex_handshake_timeout: launch.codex_handshake_timeout,
            max_concurrent_tasks: launch.max_concurrent_tasks,
            daemon_id: launch.daemon_id,
            device_name: launch.device_name,
            runtime_name: launch.runtime_name,
            profile: launch.profile,
            health_port: launch.health_port,
            allow_no_agents: launch.allow_no_agents,
            disable_auto_update: launch.disable_auto_update,
            auto_update_check_interval: launch.auto_update_check_interval,
            disable_auto_reload: launch.disable_auto_reload,
            cli_profile_overrides: Some(CliProfileConfig {
                profile_command_overrides: profile.profile_command_overrides,
                openclaw_binary_path: profile.openclaw_binary_path,
                openclaw_state_dir: profile.openclaw_state_dir,
                openclaw_cli_timeout: profile.openclaw_cli_timeout,
            }),
            cli_version,
            launched_by,
        };
        let config = config::load_config(overrides, probe_agents)?;

        // Fail closed across the entire daemon-owned tree before any provider
        // process can inherit a working directory below it.
        ensure_workspaces_root_marker(&config.workspaces_root)?;

        let config = Arc::new(config);
        let client = Arc::new(Client::new(&config.server_base_url));
        client.set_token(&profile.token);
        client.set_version(&config.cli_version);
        let repo_cache = Arc::new(Cache::new(
            std::path::Path::new(&config.workspaces_root).join(".repos"),
        ));
        let launch_registry = Arc::new(RuntimeLaunchRegistry::default());
        Ok(Self {
            config,
            client,
            repo_cache,
            launch_registry,
        })
    }

    /// Builds the concrete production provider and registration owners from
    /// one resolved profile snapshot. The caller supplies the real provider
    /// catalog; there is intentionally no metadata-only or no-op fallback.
    /// The adapter, registration source, and stack all share this input's
    /// config, authenticated client, launch registry, and checkout registry.
    pub fn into_production_assembly<C: ProviderCatalog>(
        self,
        catalog: Arc<C>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> DaemonProductionAssembly<ProductionProviderAdapter, ProviderRegistrationSource<C>> {
        let provider = Arc::new(ProductionProviderAdapter::new(Arc::clone(&self.config)));
        let registration_source = Arc::new(ProviderRegistrationSource::new(
            Arc::clone(&self.config),
            Arc::clone(&self.client),
            catalog,
            Arc::clone(&self.launch_registry),
        ));
        DaemonProductionAssembly {
            inputs: self,
            provider,
            registration_source,
            checkout_registry,
        }
    }

    /// Consumes validated inputs into the only production stack assembly
    /// path. Registration, provider execution, and checkout are mandatory
    /// shared dependencies; there is no default or no-op construction.
    pub async fn into_stack<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource>(
        self,
        provider: Arc<P>,
        registration_source: Arc<R>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> anyhow::Result<DaemonProductionStack<DaemonProductionServices<P, R>>> {
        let config = self.config;
        let services = Arc::new(DaemonProductionServices::new(
            Arc::clone(&config),
            Arc::clone(&self.client),
            Arc::clone(&self.repo_cache),
            Arc::clone(&checkout_registry),
            Arc::clone(&self.launch_registry),
            provider,
            registration_source,
        ));
        DaemonProductionStack::new_shared(
            config,
            self.client,
            self.repo_cache,
            services,
            checkout_registry,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_agent() -> BTreeMap<String, AgentEntry> {
        BTreeMap::from([(
            "codex".to_string(),
            AgentEntry {
                path: "/bin/codex".to_string(),
                command: "codex".to_string(),
                model: String::new(),
            },
        )])
    }

    #[test]
    fn foreground_args_preserve_explicit_zero_without_credentials() {
        let launch = DaemonLaunchOverrides {
            server_url: "https://cordy.example".to_string(),
            poll_interval: Duration::from_secs(3),
            agent_timeout: Some(Duration::ZERO),
            max_concurrent_tasks: 4,
            health_port: 20123,
            profile: "staging".to_string(),
            disable_auto_update: true,
            disable_auto_reload: true,
            ..DaemonLaunchOverrides::default()
        };
        let args = launch
            .foreground_args()
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(&args[..3], ["daemon", "start", "--foreground"]);
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--agent-timeout" && pair[1] == "0s"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--poll-interval" && pair[1] == "3s"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--max-concurrent-tasks" && pair[1] == "4"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--health-port" && pair[1] == "20123"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--profile" && pair[1] == "staging"));
        assert!(args.contains(&"--no-auto-update".to_string()));
        assert!(args.contains(&"--no-auto-reload".to_string()));
        assert!(!args.iter().any(|arg| arg.contains("token")));
    }

    #[test]
    fn rejects_missing_auth_before_touching_workspace_root() {
        let root = tempfile::tempdir().unwrap().path().join("not-created");
        let result = DaemonProductionInputs::resolve_with_probe(
            DaemonLaunchOverrides {
                workspaces_root: root.display().to_string(),
                ..DaemonLaunchOverrides::default()
            },
            DaemonProfileInput::default(),
            "1.2.3".to_string(),
            String::new(),
            &one_agent,
        );
        let error = match result {
            Ok(_) => panic!("missing auth unexpectedly assembled production inputs"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cordy login"));
        assert!(!root.exists());
    }

    #[test]
    fn initializes_authenticated_client_and_daemon_cache() {
        let root = tempfile::tempdir().unwrap().path().join("workspaces");
        let inputs = DaemonProductionInputs::resolve_with_probe(
            DaemonLaunchOverrides {
                server_url: "https://example.test".to_string(),
                workspaces_root: root.display().to_string(),
                allow_no_agents: false,
                ..DaemonLaunchOverrides::default()
            },
            DaemonProfileInput {
                token: "secret-token".to_string(),
                ..DaemonProfileInput::default()
            },
            "1.2.3".to_string(),
            "desktop".to_string(),
            &one_agent,
        )
        .unwrap();

        assert_eq!(inputs.client.token(), "secret-token");
        assert_eq!(inputs.config.cli_version, "1.2.3");
        assert_eq!(inputs.config.launched_by, "desktop");
        assert_eq!(inputs.config.server_base_url, "https://example.test");
        assert!(root.join(".cordy/daemon_task_context.json").is_file());
    }
}
