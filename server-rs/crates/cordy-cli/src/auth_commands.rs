//! Authentication status and logout command orchestration.
//!
//! Credential resolution remains profile-aware and task-context-aware, while
//! this module keeps auth output decisions separate from the command registry.

use anyhow::{bail, Context, Result};

use super::api::{http_timeout, ApiClient};
use super::config;
use super::{
    normalize_api_base_url, require_human_local_command, require_task_local_config_root, AuthUser,
    Cli, Environment, OutputFormat, RunOutput, CLIENT_VERSION,
};
use serde_json::Value;

pub(crate) async fn run_auth_status(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let task_context = environment.in_daemon_managed_execution_context();
    let (server_url, token) = resolve_auth_status_credentials(cli, environment)?;
    if token.is_empty() {
        return Ok(match output {
            OutputFormat::Table => RunOutput {
                stdout: String::new(),
                stderr: "Not authenticated. Run 'cordy login' to authenticate.\n".into(),
            },
            OutputFormat::Json => RunOutput {
                stdout: format!(
                    "{}\n",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "authenticated": false,
                        "server": server_url
                    }))?
                ),
                stderr: String::new(),
            },
        });
    }

    let client = ApiClient::new(
        server_url.clone(),
        String::new(),
        token.clone(),
        String::new(),
        String::new(),
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")),
        CLIENT_VERSION,
    )?;
    let user = match client.get_json::<AuthUser>("/api/me").await {
        Ok(user) => user,
        Err(error) => {
            let message = format!(
                "Token is invalid or expired: {error}\nRun 'cordy login' to re-authenticate."
            );
            return Ok(match output {
                OutputFormat::Table => RunOutput {
                    stdout: String::new(),
                    stderr: format!("{message}\n"),
                },
                OutputFormat::Json => RunOutput {
                    stdout: format!(
                        "{}\n",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "authenticated": false,
                            "server": server_url,
                            "error": message
                        }))?
                    ),
                    stderr: String::new(),
                },
            });
        }
    };
    let token_prefix = display_token_prefix(&token);
    Ok(match output {
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: if task_context {
                format!(
                    "Server:  {server_url}\nUser:    {} ({})\n",
                    user.name, user.email
                )
            } else {
                format!(
                    "Server:  {server_url}\nUser:    {} ({})\nToken:   {token_prefix}\n",
                    user.name, user.email
                )
            },
        },
        OutputFormat::Json => {
            let mut status = serde_json::json!({
                "authenticated": true,
                "server": server_url,
                "user": user
            });
            if !task_context {
                status["token"] = Value::String(token_prefix);
            }
            RunOutput {
                stdout: format!("{}\n", serde_json::to_string_pretty(&status)?),
                stderr: String::new(),
            }
        }
    })
}

pub(crate) fn run_auth_logout(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    require_human_local_command(environment, "logout")?;
    let removed = environment
        .clear_profile_token(&cli.profile)
        .context("failed to save config")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: if removed {
            "Token removed. You are now logged out.\n".into()
        } else {
            "Not authenticated.\n".into()
        },
    })
}

fn resolve_auth_status_credentials(
    cli: &Cli,
    environment: &Environment,
) -> Result<(String, String)> {
    let task_context = environment.in_daemon_managed_execution_context();
    let may_read_config =
        !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some();
    let config = if may_read_config {
        environment.load_config(&cli.profile).unwrap_or_default()
    } else {
        config::CliConfig::default()
    };
    let token = environment
        .trimmed("CORDY_TOKEN")
        .map(ToOwned::to_owned)
        .or_else(|| (!task_context).then(|| config.token.clone()))
        .unwrap_or_default();
    if task_context && !token.starts_with("mat_") {
        bail!("agent execution context requires CORDY_TOKEN to be a task-scoped mat_ token");
    }
    let explicit_server_url = cli
        .server_url
        .as_deref()
        .or_else(|| environment.trimmed("CORDY_SERVER_URL"));
    let server_url = if let Some(raw) = explicit_server_url.filter(|value| !value.is_empty()) {
        normalize_api_base_url(raw)?
    } else if may_read_config && !config.server_url.is_empty() {
        normalize_api_base_url(&config.server_url)?
    } else {
        String::new()
    };
    if server_url.is_empty() {
        bail!(
            "No server configured. Run 'cordy setup' first{}.",
            environment.daemon_port_only_context_hint()
        );
    }
    Ok((server_url, token))
}

pub(crate) fn display_token_prefix(token: &str) -> String {
    if token.chars().count() > 12 {
        token.chars().take(12).collect::<String>() + "..."
    } else {
        token.into()
    }
}
