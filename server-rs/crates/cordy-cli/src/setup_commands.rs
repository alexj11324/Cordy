//! Setup command orchestration.
//!
//! Setup combines profile resolution, health-before-write validation, browser
//! login, and daemon handoff. Keeping that policy in one module leaves the
//! root dispatcher focused on command routing while preserving each existing
//! persistence and lifecycle boundary.

use anyhow::{bail, Context, Result};
use std::io::{Read, Write as IoWrite};
use std::time::Duration;
use url::Url;

use super::config::Environment;
use super::{
    config, normalize_api_base_url, require_human_local_command, run_daemon_after_setup,
    run_login_with_urls, ApiClient, Cli, RunOutput, SetupArgs, SetupCommand, SetupError,
    CLOUD_APP_URL, CLOUD_SERVER_URL,
};

const SETUP_HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn run_setup<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SetupArgs,
    input: &mut R,
) -> Result<RunOutput> {
    require_human_local_command(environment, "setup")?;
    let setup_input = resolve_setup_profile_input(cli, environment, args)?;
    if !confirm_setup_overwrite(cli, environment, &setup_input, input)? {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "Aborted.\n".into(),
        });
    }
    let input = prepare_setup_profile_input(environment, &cli.profile, setup_input).await?;
    let mut output = if environment.trimmed("CORDY_TOKEN").is_some() {
        run_daemon_after_setup(cli, environment).await?
    } else {
        let login_output = run_login_with_urls(
            cli,
            environment,
            &super::LoginArgs {
                token: None,
                callback_host: setup_callback_host(args),
            },
            Some(&input.server_url),
            Some(&input.app_url),
        )
        .await?;
        let mut daemon_output = run_daemon_after_setup(cli, environment).await?;
        daemon_output.stderr = format!("{}{}", login_output.stderr, daemon_output.stderr);
        daemon_output
    };
    output.stderr = format!(
        "Configured {} for profile {:?}; token authentication preserved.\n{}",
        input.server_url, cli.profile, output.stderr
    );
    Ok(output)
}

/// Ask before replacing an existing deployment profile. Persisted credentials
/// and workspace identity are deliberately omitted from the prompt.
pub(crate) fn confirm_setup_overwrite<R: Read>(
    cli: &Cli,
    environment: &Environment,
    target: &config::SetupProfileInput,
    input: &mut R,
) -> Result<bool> {
    let existing = environment.load_config(&cli.profile).unwrap_or_default();
    if existing.server_url.trim().is_empty() {
        return Ok(true);
    }

    let prompt = format!(
        "Existing configuration for profile {:?} will be replaced:\n  server_url: {}\n  app_url:    {}\nContinue? [y/N] ",
        cli.profile,
        format_setup_value_change(&existing.server_url, &target.server_url),
        format_setup_value_change(&existing.app_url, &target.app_url),
    );
    let mut stderr = std::io::stderr();
    stderr
        .write_all(prompt.as_bytes())
        .context("write setup overwrite prompt")?;
    stderr.flush().context("flush setup overwrite prompt")?;

    let answer = read_setup_confirmation(input)?;
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

pub(crate) fn format_setup_value_change(old: &str, new: &str) -> String {
    if old == new {
        old.to_owned()
    } else {
        format!("{old}  ->  {new}")
    }
}

pub(crate) fn read_setup_confirmation<R: Read>(input: &mut R) -> Result<String> {
    const MAX_CONFIRMATION_BYTES: usize = 4096;
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while answer.len() < MAX_CONFIRMATION_BYTES {
        let read = input
            .read(&mut byte)
            .context("read setup overwrite confirmation")?;
        if read == 0 || byte[0] == b'\n' {
            break;
        }
        answer.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&answer).trim().to_ascii_lowercase())
}

pub(crate) async fn prepare_setup_profile(
    cli: &Cli,
    environment: &Environment,
    args: &SetupArgs,
) -> Result<config::SetupProfileInput> {
    require_human_local_command(environment, "setup")?;
    let input = resolve_setup_profile_input(cli, environment, args)?;
    prepare_setup_profile_input(environment, &cli.profile, input).await
}

pub(crate) async fn prepare_setup_profile_input(
    environment: &Environment,
    profile: &str,
    input: config::SetupProfileInput,
) -> Result<config::SetupProfileInput> {
    ApiClient::probe_health(&input.server_url, SETUP_HEALTH_TIMEOUT)
        .await
        .map_err(SetupError::HealthProbe)?;

    if let Some(token) = environment.trimmed("CORDY_TOKEN") {
        environment.replace_profile_for_setup(profile, &input)?;
        environment.set_profile_value(
            profile,
            "token",
            Some(serde_json::Value::String(token.to_owned())),
        )?;
    }
    Ok(input)
}

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
