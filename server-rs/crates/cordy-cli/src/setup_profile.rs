//! Setup profile resolution and daemon handoff policy.
//!
//! This module keeps setup's deployment selection and running-daemon policy
//! separate from health probing, login, and locked persistence orchestration.

use anyhow::{bail, Result};
use url::Url;

use crate::{
    config, normalize_api_base_url, Cli, Environment, RunOutput, SetupArgs, SetupCommand,
    SetupError, CLOUD_APP_URL, CLOUD_SERVER_URL,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupDaemonAction {
    Start,
    Restart,
    LeaveRunning { active_task_count: i64 },
}

pub(crate) fn setup_daemon_action(
    daemon_running: bool,
    active_task_count: i64,
) -> SetupDaemonAction {
    if daemon_running {
        if active_task_count > 0 {
            return SetupDaemonAction::LeaveRunning { active_task_count };
        }
        return SetupDaemonAction::Restart;
    }
    SetupDaemonAction::Start
}

pub(crate) async fn dispatch_daemon_after_setup<S, R, SFut, RFut>(
    action: SetupDaemonAction,
    start: S,
    restart: R,
) -> Result<RunOutput>
where
    S: FnOnce() -> SFut,
    R: FnOnce() -> RFut,
    SFut: std::future::Future<Output = Result<RunOutput>>,
    RFut: std::future::Future<Output = Result<RunOutput>>,
{
    match action {
        SetupDaemonAction::Start => start().await,
        SetupDaemonAction::Restart => restart().await,
        SetupDaemonAction::LeaveRunning { active_task_count } => {
            let task_label = if active_task_count == 1 {
                "task"
            } else {
                "tasks"
            };
            let restart_command = "cordy daemon restart";
            bail!(
                "daemon has {active_task_count} active {task_label}; setup saved the new configuration but left the running daemon unchanged to avoid cancelling work. Wait for the active work to finish, then run '{restart_command}' to apply the new configuration"
            )
        }
    }
}

pub(crate) fn resolve_setup_profile_input(
    cli: &Cli,
    environment: &Environment,
    args: &SetupArgs,
) -> Result<config::SetupProfileInput> {
    let existing = environment.load_config(&cli.profile).unwrap_or_default();
    match &args.command {
        None | Some(SetupCommand::Cloud(_)) => {
            config::SetupProfileInput::new(CLOUD_SERVER_URL, CLOUD_APP_URL)
        }
        Some(SetupCommand::SelfHost(options)) => {
            let raw_server = cli
                .server_url
                .as_deref()
                .or_else(|| environment.trimmed("CORDY_SERVER_URL"))
                .or_else(|| {
                    (!existing.server_url.trim().is_empty()).then_some(existing.server_url.as_str())
                })
                .unwrap_or("");
            let raw_server = if raw_server.is_empty() {
                format!("http://localhost:{}", options.port)
            } else {
                raw_server.to_owned()
            };
            let server_url = normalize_api_base_url(&raw_server)?;
            let app_url = options
                .app_url
                .as_deref()
                .or_else(|| environment.trimmed("CORDY_APP_URL"))
                .or_else(|| {
                    (!existing.app_url.trim().is_empty()).then_some(existing.app_url.as_str())
                })
                .map(|value| value.trim_end_matches('/').to_owned())
                .or_else(|| {
                    setup_server_is_local(&server_url)
                        .then(|| format!("http://localhost:{}", options.frontend_port))
                })
                .ok_or(SetupError::RemoteAppUrlRequired)?;
            config::SetupProfileInput::new(server_url, app_url)
        }
    }
}

pub(crate) fn setup_callback_host(args: &SetupArgs) -> Option<String> {
    match args.command.as_ref() {
        Some(SetupCommand::Cloud(options)) => options.callback_host.clone(),
        Some(SetupCommand::SelfHost(options)) => options.callback_host.clone(),
        None => args.callback_host.clone(),
    }
}

pub(crate) fn setup_server_is_local(server_url: &str) -> bool {
    let Ok(url) = Url::parse(server_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}
