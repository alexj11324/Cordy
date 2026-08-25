//! Output rendering for daemon health/status inspection.
//!
//! Keeping presentation separate from profile and health orchestration makes
//! the status command reusable by lifecycle commands without coupling them to
//! table/JSON formatting details.

use anyhow::Result;
use std::fmt::Write as FmtWrite;

use super::{OutputFormat, RunOutput};

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
