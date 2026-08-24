//! Production daemon command input assembly.
//!
//! Start and restart both use this owner so the background child, foreground
//! bootstrap, and self-update successor receive one identical resolved launch
//! configuration. Provider construction remains mandatory at the foreground
//! `run_production_daemon` boundary; this module does not install a fallback.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cordy_daemon::assembly::{
    DaemonLaunchOverrides, DaemonProductionAssembly, DaemonProductionInputs, DaemonProfileInput,
};
use cordy_daemon::bootstrap::{BootstrapContext, BootstrapOptions};
use cordy_daemon::health::RepoCheckoutRegistry;
use cordy_daemon::lifecycle::DaemonLifecycleOptions;
use cordy_daemon::provider_adapter::ProductionProviderAdapter;
use cordy_daemon::provider_registration::{
    LocalProviderCatalog, ProviderCatalog, ProviderRegistrationSource,
};

use crate::config::{resolve_daemon_launch_overrides, CliConfig, DaemonLaunchFlags, Environment};

/// Authenticated inputs shared by background start/restart and the foreground
/// production daemon. This type intentionally has no `Debug` implementation:
/// `profile_input` contains the stored bearer token.
#[derive(Clone)]
pub struct DaemonStartAssembly {
    pub launch: DaemonLaunchOverrides,
    pub profile_input: DaemonProfileInput,
}

impl DaemonStartAssembly {
    /// Loads one profile snapshot and resolves every launch-precedence layer.
    /// Authentication fails before a background process can be spawned.
    pub fn load(
        profile: &str,
        flags: &DaemonLaunchFlags,
        environment: &Environment,
    ) -> Result<Self> {
        if environment.in_daemon_managed_execution_context() {
            bail!("daemon start and restart are not available inside a daemon-managed task");
        }
        let config = environment.load_config(profile)?;
        Self::from_config(profile, flags, environment, &config)
    }

    /// Loads the local profile for a stop operation without requiring a
    /// bearer token. Stopping is a local PID/health transaction; requiring a
    /// server credential here would make it impossible to stop a daemon after
    /// an expired or revoked login. Restart and start continue to use
    /// [`Self::load`] so their authenticated preflight remains fail-closed.
    pub fn load_for_control(
        profile: &str,
        flags: &DaemonLaunchFlags,
        environment: &Environment,
    ) -> Result<Self> {
        if environment.in_daemon_managed_execution_context() {
            bail!("daemon lifecycle commands are not available inside a daemon-managed task");
        }
        let config = environment.load_config(profile)?;
        let launch = resolve_daemon_launch_overrides(profile, flags, environment, &config)?;
        Ok(Self {
            launch,
            profile_input: config.daemon_profile_input(),
        })
    }

    fn from_config(
        profile: &str,
        flags: &DaemonLaunchFlags,
        environment: &Environment,
        config: &CliConfig,
    ) -> Result<Self> {
        let profile_input = config.daemon_profile_input();
        if profile_input.token.is_empty() {
            let login = if profile.is_empty() {
                "cordy login".to_string()
            } else {
                format!("cordy login --profile {profile}")
            };
            bail!("not authenticated: run '{login}' first");
        }
        let launch = resolve_daemon_launch_overrides(profile, flags, environment, config)?;
        Ok(Self {
            launch,
            profile_input,
        })
    }

    /// Background lifecycle input. The lifecycle owner performs authenticated
    /// preflight before spawn and waits for foreground readiness.
    pub fn lifecycle_options(
        &self,
        executable: PathBuf,
        cli_version: impl Into<String>,
    ) -> DaemonLifecycleOptions {
        DaemonLifecycleOptions::new(executable, self.launch.clone(), cli_version)
    }

    /// Foreground bootstrap input, including the canonical successor argv used
    /// by both auto-update and reload handoff.
    pub fn bootstrap_options(&self) -> BootstrapOptions {
        BootstrapOptions::new(self.launch.profile.clone(), self.launch.foreground_args())
    }

    /// Foreground production input. The bootstrap context is the authoritative
    /// source for launcher identity, so the foreground daemon cannot silently
    /// diverge from the process-level value used by successor handoff and
    /// health registration.
    pub fn production_inputs(
        &self,
        context: &BootstrapContext,
        cli_version: impl Into<String>,
    ) -> Result<DaemonProductionInputs> {
        DaemonProductionInputs::resolve(
            self.launch.clone(),
            self.profile_input.clone(),
            cli_version,
            context.launched_by.clone(),
        )
        .context("assemble foreground daemon production inputs")
    }

    /// Completes the typed foreground assembly once the command layer has a
    /// real provider catalog. The catalog is mandatory: CLI wiring must not
    /// manufacture a placeholder adapter for an unsupported provider family.
    pub fn production_assembly<C: ProviderCatalog>(
        &self,
        context: &BootstrapContext,
        cli_version: impl Into<String>,
        catalog: Arc<C>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> Result<DaemonProductionAssembly<ProductionProviderAdapter, ProviderRegistrationSource<C>>>
    {
        let inputs = self.production_inputs(context, cli_version)?;
        Ok(inputs.into_production_assembly(catalog, checkout_registry))
    }

    /// Completes foreground assembly with the daemon's real local catalog.
    /// This is the command-facing entry point once the CLI command supplies
    /// its bootstrap context and checkout registry; it performs no metadata
    /// fallback for provider families without a landed backend.
    pub fn production_assembly_with_local_catalog(
        &self,
        context: &BootstrapContext,
        cli_version: impl Into<String>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> Result<
        DaemonProductionAssembly<
            ProductionProviderAdapter,
            ProviderRegistrationSource<LocalProviderCatalog>,
        >,
    > {
        self.production_assembly(
            context,
            cli_version,
            Arc::new(LocalProviderCatalog::new()),
            checkout_registry,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;

    #[test]
    fn one_profile_snapshot_drives_lifecycle_and_successor_inputs() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let profile = "production";
        let path = environment.config_path(profile).expect("config path");
        fs::create_dir_all(path.parent().expect("profile directory")).expect("create profile");
        fs::write(
            path,
            r#"{
                "token":"mul_secret",
                "server_url":"https://profile.example",
                "device_name":"profile-device",
                "poll_interval":"3s",
                "profile_command_overrides":{"profile-1":"/opt/custom-agent"},
                "backends":{"openclaw":{"state_dir":"/srv/openclaw"}}
            }"#,
        )
        .expect("write profile");

        let assembly =
            DaemonStartAssembly::load(profile, &DaemonLaunchFlags::default(), &environment)
                .expect("assemble daemon start");
        assert_eq!(assembly.launch.server_url, "https://profile.example");
        assert_eq!(assembly.launch.device_name, "profile-device");
        assert_eq!(assembly.launch.poll_interval, Duration::from_secs(3));
        assert_eq!(assembly.profile_input.token, "mul_secret");
        assert_eq!(assembly.profile_input.openclaw_state_dir, "/srv/openclaw");

        let executable = PathBuf::from("/opt/cordy/bin/cordy");
        let lifecycle = assembly.lifecycle_options(executable.clone(), "v1.2.3");
        let bootstrap = assembly.bootstrap_options();
        assert_eq!(lifecycle.executable, executable);
        assert_eq!(lifecycle.cli_version, "v1.2.3");
        assert_eq!(lifecycle.launch.foreground_args(), bootstrap.successor_args);
        assert_eq!(bootstrap.profile, profile);
    }

    #[test]
    fn start_fails_before_spawn_without_profile_credentials() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let error =
            DaemonStartAssembly::load("missing", &DaemonLaunchFlags::default(), &environment)
                .err()
                .expect("missing token must fail");
        assert!(error.to_string().contains("cordy login --profile missing"));
    }

    #[test]
    fn stop_profile_load_does_not_require_server_credentials() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let assembly = DaemonStartAssembly::load_for_control(
            "missing",
            &DaemonLaunchFlags::default(),
            &environment,
        )
        .expect("local stop should not require login");
        assert!(assembly.profile_input.token.is_empty());
    }

    #[test]
    fn daemon_managed_tasks_cannot_assemble_nested_daemons() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_DAEMON_PORT", "19876");

        let error = DaemonStartAssembly::load("", &DaemonLaunchFlags::default(), &environment)
            .err()
            .expect("nested daemon must fail");
        assert!(error.to_string().contains("daemon-managed task"));
    }

    #[test]
    fn foreground_assembly_rejects_missing_auth_before_touching_workspace_root() {
        let root = tempfile::tempdir()
            .expect("temp root")
            .path()
            .join("not-created");
        let context = BootstrapContext {
            paths: cordy_daemon::bootstrap::ProfileStatePaths {
                directory: root.join("daemon"),
                pid: root.join("daemon/pid"),
                pid_lock: root.join("daemon/pid.lock"),
                structured_log: root.join("daemon/daemon.log"),
                crash_log: root.join("daemon/daemon.err.log"),
            },
            launched_by: "desktop".to_string(),
            shutdown: tokio_util::sync::CancellationToken::new(),
        };
        let assembly = DaemonStartAssembly {
            launch: DaemonLaunchOverrides {
                workspaces_root: root.display().to_string(),
                ..DaemonLaunchOverrides::default()
            },
            profile_input: DaemonProfileInput::default(),
        };

        let error = assembly
            .production_inputs(&context, "1.2.3")
            .expect_err("foreground assembly must require profile credentials");
        assert!(error.to_string().contains("cordy login"));
        assert!(!root.exists());
    }
}
