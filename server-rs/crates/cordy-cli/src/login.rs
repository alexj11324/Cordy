//! Login credential verification and profile persistence.
//!
//! Browser callback transport and workspace discovery live in
//! `login_browser`; this module keeps credential selection, verification,
//! and atomic profile update orchestration together.

pub(crate) use crate::login_browser::{
    build_login_url, build_workspace_creation_url, callback_host_is_loopback, constant_time_equal,
    run_browser_login, validate_login_token, wait_for_login_callback, wait_for_workspace_creation,
    wait_for_workspace_creation_with_opener, LoginWorkspace, LOGIN_CALLBACK_TIMEOUT,
    WORKSPACE_DISCOVERY_INTERVAL, WORKSPACE_DISCOVERY_TIMEOUT,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::api::{http_timeout, ApiClient};
use super::config::Environment;
use super::{
    normalize_api_base_url, require_human_local_command, Cli, LoginArgs, RunOutput, CLIENT_VERSION,
    CLOUD_APP_URL, CLOUD_SERVER_URL,
};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct AuthUser {
    pub(crate) name: String,
    pub(crate) email: String,
}

/// Authenticate either with a PAT or with the browser callback flow. The
/// credential and workspace reset are committed together after the new token
/// has been verified, so a failed login cannot damage an existing profile.
pub(super) async fn run_login(
    cli: &Cli,
    environment: &Environment,
    args: &LoginArgs,
) -> Result<RunOutput> {
    run_login_with_urls(cli, environment, args, None, None).await
}

pub(super) async fn run_login_with_urls(
    cli: &Cli,
    environment: &Environment,
    args: &LoginArgs,
    server_override: Option<&str>,
    app_override: Option<&str>,
) -> Result<RunOutput> {
    require_human_local_command(environment, "login")?;
    let existing = environment.load_config(&cli.profile).unwrap_or_default();
    let server_url = server_override
        .or(cli.server_url.as_deref())
        .or_else(|| environment.trimmed("CORDY_SERVER_URL"))
        .filter(|value| !value.trim().is_empty())
        .map(normalize_api_base_url)
        .transpose()?
        .or_else(|| (!existing.server_url.trim().is_empty()).then(|| existing.server_url.clone()))
        .context("No server configured. Run 'cordy setup' first.")?;
    let app_url = app_override
        .map(str::to_owned)
        .or_else(|| environment.trimmed("CORDY_APP_URL").map(str::to_owned))
        .or_else(|| (!existing.app_url.trim().is_empty()).then(|| existing.app_url.clone()))
        .or_else(|| (server_url == CLOUD_SERVER_URL).then(|| CLOUD_APP_URL.into()))
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned();

    let token = match args
        .token
        .as_deref()
        .map(str::trim)
        .or_else(|| environment.trimmed("CORDY_TOKEN"))
    {
        Some(token) if !token.is_empty() => {
            validate_login_token(token)?;
            token.to_owned()
        }
        _ => {
            run_browser_login(
                &server_url,
                &app_url,
                args.callback_host.as_deref(),
                environment,
            )
            .await?
        }
    };

    let client = ApiClient::new(
        server_url.clone(),
        String::new(),
        token.clone(),
        String::new(),
        String::new(),
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")),
        CLIENT_VERSION,
    )?;
    let user = client
        .get_json::<AuthUser>("/api/me")
        .await
        .map_err(|_| anyhow::anyhow!("could not verify the new credential"))?;
    let workspaces = client
        .get_json::<Vec<LoginWorkspace>>("/api/workspaces")
        .await
        .map_err(|_| anyhow::anyhow!("workspace discovery request failed"));
    let workspaces = match workspaces {
        Ok(workspaces) if !workspaces.is_empty() => Ok(workspaces),
        Ok(_) => {
            wait_for_workspace_creation(
                &client,
                &app_url,
                WORKSPACE_DISCOVERY_INTERVAL,
                WORKSPACE_DISCOVERY_TIMEOUT,
            )
            .await
        }
        Err(error) => Err(error),
    };
    let (workspace_id, workspace_message) = match workspaces {
        Ok(workspaces) => {
            let selected = workspaces
                .first()
                .filter(|workspace| !workspace.id.is_empty());
            let message = format!(
                "Found {} workspace(s); default workspace reset to {}.\n",
                workspaces.len(),
                selected
                    .map(|workspace| workspace.name.as_str())
                    .unwrap_or("the first workspace")
            );
            (
                selected
                    .map(|workspace| workspace.id.clone())
                    .unwrap_or_default(),
                message,
            )
        }
        Err(_) => {
            // Authentication is still valid when discovery is temporarily
            // unavailable. Persist an empty workspace and make the retry
            // actionable without printing any bearer material.
            (
                String::new(),
                "Authenticated, but workspace discovery did not complete; run 'cordy workspace list' to retry.\n".into(),
            )
        }
    };
    environment.save_authenticated_profile(
        &cli.profile,
        &server_url,
        &app_url,
        &token,
        &workspace_id,
    )?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!(
            "Authenticated as {} ({}).\nToken saved to config.\n{}",
            user.name, user.email, workspace_message
        ),
    })
}
