//! Command-facing daemon health/status inspection.
//!
//! Profile resolution and status presentation stay together so lifecycle
//! commands can reuse the same profile validation without owning output code.

use anyhow::{bail, Context, Result};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::Path;

use super::config::Environment;
use super::{Cli, DaemonStatusArgs, OutputFormat, RunOutput};

pub(crate) async fn run_daemon_status(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonStatusArgs,
) -> Result<RunOutput> {
    let port = resolve_daemon_status_port(cli, environment)?;
    let control = cordy_daemon::control_client::DaemonControlClient::try_new()
        .context("build daemon health client")?;
    let health = control.health(port).await;
    let conflict = if environment.in_daemon_task_identity_context() {
        None
    } else if let cordy_daemon::control_client::LocalDaemonHealth::Live(snapshot) = &health {
        snapshot.confirm_profile(&cli.profile, port).err()
    } else {
        None
    };
    render_daemon_status(&cli.profile, args.output, health, conflict)
}

pub(crate) fn resolve_daemon_status_port(cli: &Cli, environment: &Environment) -> Result<u16> {
    if !environment.in_daemon_task_identity_context() {
        require_known_daemon_profile(environment, &cli.profile)?;
        return Ok(cordy_daemon::control_client::health_port_for_profile(
            &cli.profile,
        ));
    }

    if !cli.profile.is_empty() {
        bail!("daemon status --profile is not available inside a daemon-managed task");
    }
    let raw = environment.trimmed("CORDY_DAEMON_PORT").context(
        "daemon status inside a daemon-managed task requires the daemon-injected CORDY_DAEMON_PORT",
    )?;
    raw.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("invalid CORDY_DAEMON_PORT inside a daemon-managed task")
}

pub(crate) fn require_known_daemon_profile(environment: &Environment, profile: &str) -> Result<()> {
    if profile.is_empty() {
        return Ok(());
    }
    let config_path = environment.config_path(profile)?;
    let profile_dir = config_path
        .parent()
        .context("resolve daemon profile directory")?;
    if profile_dir.is_dir() {
        return Ok(());
    }

    let known = known_daemon_profiles(environment);
    if known.is_empty() {
        bail!("unknown profile {profile:?}: no named profiles exist yet");
    }
    bail!(
        "unknown profile {profile:?}\nKnown profiles: {}",
        known.join(", ")
    );
}

pub(crate) fn known_daemon_profiles(environment: &Environment) -> Vec<String> {
    let Ok(config_path) = environment.config_path("") else {
        return Vec::new();
    };
    let Some(config_dir) = config_path.parent() else {
        return Vec::new();
    };
    let profiles_root = config_dir.join("profiles");
    let mut names = Vec::new();
    collect_daemon_profiles(&profiles_root, Path::new(""), &mut names);
    names.sort();
    names
}

fn collect_daemon_profiles(root: &Path, relative: &Path, names: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child_relative = relative.join(entry.file_name());
        let child = entry.path();
        if child.join("config.json").is_file() {
            names.push(child_relative.to_string_lossy().replace('\\', "/"));
        }
        collect_daemon_profiles(&child, &child_relative, names);
    }
}

pub(crate) fn render_daemon_status(
    profile: &str,
    output: OutputFormat,
    health: cordy_daemon::control_client::LocalDaemonHealth,
    conflict: Option<cordy_daemon::control_client::ProfileMismatch>,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        let value = if let Some(conflict) = conflict.as_ref() {
            let port_conflict = match &conflict.actual {
                Some(actual) => serde_json::json!({
                    "port": conflict.port,
                    "profile": actual,
                }),
                None => serde_json::json!({
                    "port": conflict.port,
                    "unreadable_identity": true,
                }),
            };
            serde_json::json!({
                "status": "stopped",
                "port_conflict": port_conflict,
            })
        } else {
            match health {
                cordy_daemon::control_client::LocalDaemonHealth::Stopped => {
                    serde_json::json!({ "status": "stopped" })
                }
                cordy_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
                    serde_json::to_value(snapshot.response)?
                }
            }
        };
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&value)?),
            stderr: String::new(),
        });
    }

    let label = daemon_status_label(profile);
    let stdout = if let Some(conflict) = conflict.as_ref() {
        format!(
            "{label}: stopped\n{}\n",
            daemon_status_conflict_note(conflict)
        )
    } else {
        match health {
            cordy_daemon::control_client::LocalDaemonHealth::Stopped => {
                format!("{label}: stopped\n")
            }
            cordy_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
                format_daemon_status_table(&label, &snapshot.response)
            }
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn daemon_status_label(profile: &str) -> String {
    if profile.is_empty() {
        "Daemon".to_string()
    } else {
        format!("Daemon [{profile}]")
    }
}

fn daemon_status_conflict_note(conflict: &cordy_daemon::control_client::ProfileMismatch) -> String {
    match &conflict.actual {
        Some(actual) => format!(
            "Note: port {} is serving {:?}, which hashes to the same port.",
            conflict.port, actual
        ),
        None => format!(
            "Note: port {} is serving a daemon whose profile identity could not be read.",
            conflict.port
        ),
    }
}

pub(crate) fn format_daemon_status_table(
    label: &str,
    response: &cordy_daemon::health::HealthResponse,
) -> String {
    let mut rows = vec![(
        label.to_string(),
        format!(
            "{} (pid {}, uptime {})",
            response.status, response.pid, response.uptime
        ),
    )];
    if !response.cli_version.is_empty() {
        rows.push(("Version".to_string(), response.cli_version.clone()));
    }
    if !response.launched_by.is_empty() {
        let manager = if response.launched_by == "desktop" {
            "Cordy Desktop app (start and stop it from the app)".to_string()
        } else {
            response.launched_by.clone()
        };
        rows.push(("Managed by".to_string(), manager));
    }
    if !response.reload_pending_reason.is_empty() {
        rows.push((
            "Restart pending".to_string(),
            response.reload_pending_reason.clone(),
        ));
    }
    if !response.agents.is_empty() {
        rows.push(("Agents".to_string(), response.agents.join(", ")));
    }
    rows.push((
        "Workspaces".to_string(),
        response.workspaces.len().to_string(),
    ));

    let width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0) + 1;
    let mut output = String::new();
    for (key, value) in rows {
        let key = format!("{key}:");
        let _ = writeln!(output, "{key:<width$}  {value}", width = width);
    }
    output
}
