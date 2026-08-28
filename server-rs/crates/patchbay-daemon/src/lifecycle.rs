//! Production daemon lifecycle facade consumed by `patchbay-cli`.
//!
//! CLI parsing and profile persistence remain in the CLI crate. Once those
//! values are resolved, this owner is the single start/stop/restart assembly
//! path and therefore cannot drift from daemon bootstrap semantics.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::assembly::{DaemonLaunchOverrides, DaemonProfileInput};
use crate::client::Client;
use crate::config::{normalize_server_base_url, DEFAULT_SERVER_URL};
use crate::control_client::{health_port_for_profile, DaemonControlClient};
use crate::process_control::{
    restart_daemon, start_daemon, stop_daemon, stop_daemon_with_preflight,
    AuthenticatedLaunchPreflight, BackgroundLaunchOptions, DaemonRestartOutcome,
    DaemonRestartRequest, DaemonStartOutcome, DaemonStartRequest, DaemonStopOutcome,
    SystemProcessTerminator, SystemStartupClock,
};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DaemonLifecycleOptions {
    pub executable: PathBuf,
    pub launch: DaemonLaunchOverrides,
    pub cli_version: String,
    pub startup_timeout: Duration,
    pub stop_timeout: Duration,
}

impl DaemonLifecycleOptions {
    pub fn new(
        executable: PathBuf,
        launch: DaemonLaunchOverrides,
        cli_version: impl Into<String>,
    ) -> Self {
        Self {
            executable,
            launch,
            cli_version: cli_version.into(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
        }
    }
}

pub struct DaemonLifecycle {
    control: DaemonControlClient,
    preflight: AuthenticatedLaunchPreflight,
    launch: BackgroundLaunchOptions,
    port: u16,
    startup_timeout: Duration,
    stop_timeout: Duration,
}

impl DaemonLifecycle {
    /// Builds production lifecycle ownership without probing agents or
    /// touching the workspace tree. Those expensive checks belong to the
    /// foreground child and remain its readiness gate.
    pub fn assemble(
        options: DaemonLifecycleOptions,
        profile: &DaemonProfileInput,
    ) -> anyhow::Result<Self> {
        let server_url = if options.launch.server_url.trim().is_empty() {
            DEFAULT_SERVER_URL
        } else {
            options.launch.server_url.trim()
        };
        let server_url = normalize_server_base_url(server_url)?;
        let client = Arc::new(Client::new(server_url));
        client.set_token(&profile.token);
        client.set_version(&options.cli_version);
        let port = match options.launch.health_port {
            0 => health_port_for_profile(&options.launch.profile),
            configured => u16::try_from(configured)
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(|| anyhow::anyhow!("health port is outside 1..=65535"))?,
        };
        anyhow::ensure!(
            !options.startup_timeout.is_zero(),
            "daemon startup timeout is zero"
        );
        let args = options.launch.foreground_args();
        let launch = BackgroundLaunchOptions {
            profile: options.launch.profile.clone(),
            binary: options.executable,
            args,
        };
        Ok(Self {
            control: DaemonControlClient::try_new()?,
            preflight: AuthenticatedLaunchPreflight::new(client, &options.launch.profile),
            launch,
            port,
            startup_timeout: options.startup_timeout,
            stop_timeout: options.stop_timeout,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn start(&self) -> anyhow::Result<DaemonStartOutcome> {
        start_daemon(
            &self.control,
            &SystemStartupClock,
            &self.preflight,
            DaemonStartRequest {
                launch: self.launch.clone(),
                port: self.port,
                startup_timeout: self.startup_timeout,
            },
        )
        .await
    }

    pub async fn stop(&self) -> anyhow::Result<DaemonStopOutcome> {
        stop_daemon(
            &self.control,
            &SystemStartupClock,
            &SystemProcessTerminator,
            &self.launch.profile,
            self.port,
            self.stop_timeout,
        )
        .await
    }

    /// Stops with the restart preflight while leaving the actual foreground
    /// start to the caller. This preserves `daemon restart --foreground`
    /// without risking a healthy daemon when its replacement cannot start.
    pub async fn stop_for_restart(&self) -> anyhow::Result<DaemonStopOutcome> {
        stop_daemon_with_preflight(
            &self.control,
            &SystemStartupClock,
            &SystemProcessTerminator,
            &self.launch.profile,
            self.port,
            self.stop_timeout,
            Some(&self.preflight),
        )
        .await
    }

    pub async fn restart(&self) -> anyhow::Result<DaemonRestartOutcome> {
        restart_daemon(
            &self.control,
            &SystemStartupClock,
            &SystemProcessTerminator,
            &self.preflight,
            DaemonRestartRequest {
                launch: self.launch.clone(),
                port: self.port,
                stop_timeout: self.stop_timeout,
                startup_timeout: self.startup_timeout,
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_uses_profile_port_and_never_places_token_in_argv() {
        let lifecycle = DaemonLifecycle::assemble(
            DaemonLifecycleOptions::new(
                std::env::current_exe().unwrap(),
                DaemonLaunchOverrides {
                    profile: "ab".to_string(),
                    server_url: "https://patchbay.example/ws".to_string(),
                    ..DaemonLaunchOverrides::default()
                },
                "1.2.3",
            ),
            &DaemonProfileInput {
                token: "super-secret".to_string(),
                ..DaemonProfileInput::default()
            },
        )
        .unwrap();
        assert_eq!(lifecycle.port(), health_port_for_profile("ab"));
        assert!(!lifecycle
            .launch
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("super-secret")));
    }

    #[test]
    fn explicit_health_port_is_range_checked() {
        let result = DaemonLifecycle::assemble(
            DaemonLifecycleOptions::new(
                std::env::current_exe().unwrap(),
                DaemonLaunchOverrides {
                    health_port: i32::from(u16::MAX) + 1,
                    ..DaemonLaunchOverrides::default()
                },
                "1.2.3",
            ),
            &DaemonProfileInput::default(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn explicit_health_port_is_forwarded_to_foreground_child() {
        let lifecycle = DaemonLifecycle::assemble(
            DaemonLifecycleOptions::new(
                std::env::current_exe().unwrap(),
                DaemonLaunchOverrides {
                    health_port: 20123,
                    ..DaemonLaunchOverrides::default()
                },
                "1.2.3",
            ),
            &DaemonProfileInput::default(),
        )
        .unwrap();

        assert_eq!(lifecycle.port(), 20123);
        assert!(lifecycle.launch.args.windows(2).any(|pair| {
            pair[0] == std::ffi::OsStr::new("--health-port")
                && pair[1] == std::ffi::OsStr::new("20123")
        }));
    }
}
