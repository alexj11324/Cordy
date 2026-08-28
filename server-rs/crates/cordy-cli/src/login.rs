//! Login-specific browser callback and workspace-discovery flow.
//!
//! Keeping this protocol in its own module makes the command dispatcher
//! responsible only for profile selection and output composition. The HTTP
//! client, callback validation, and bounded workspace polling remain the same
//! behavior as the original command implementation.

use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use super::api::{http_timeout, ApiClient};
use super::config::Environment;
use super::{
    normalize_api_base_url, require_human_local_command, Cli, LoginArgs, RunOutput, CLIENT_VERSION,
    CLOUD_APP_URL, CLOUD_SERVER_URL,
};

#[derive(Debug, Deserialize)]
pub(crate) struct LoginWorkspace {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
}

pub(crate) const LOGIN_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const LOGIN_CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const WORKSPACE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub(crate) const WORKSPACE_DISCOVERY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) async fn wait_for_workspace_creation(
    client: &ApiClient,
    app_url: &str,
    poll_interval: Duration,
    max_wait: Duration,
) -> Result<Vec<LoginWorkspace>> {
    wait_for_workspace_creation_with_opener(client, app_url, poll_interval, max_wait, |url| {
        if !open_login_url(url) {
            eprintln!("Could not open browser automatically.");
        }
    })
    .await
}

pub(crate) async fn wait_for_workspace_creation_with_opener<F>(
    client: &ApiClient,
    app_url: &str,
    poll_interval: Duration,
    max_wait: Duration,
    open: F,
) -> Result<Vec<LoginWorkspace>>
where
    F: FnOnce(&str),
{
    let creation_url = build_workspace_creation_url(app_url)?;
    eprintln!("No workspaces found. Opening workspace creation in your browser...");
    open(&creation_url);
    eprintln!("If the browser did not open, visit:\n  {creation_url}");
    eprintln!("\nWaiting for workspace creation...");

    let deadline = tokio::time::Instant::now() + max_wait;
    loop {
        tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + poll_interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!("timed out waiting for workspace creation"));
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let request_timeout = remaining.min(poll_interval.max(Duration::from_secs(10)));
        let workspaces = tokio::time::timeout(
            request_timeout,
            client.get_json::<Vec<LoginWorkspace>>("/api/workspaces"),
        )
        .await;
        if let Ok(Ok(workspaces)) = workspaces {
            if !workspaces.is_empty() {
                return Ok(workspaces);
            }
        }
    }
}

pub(crate) fn build_workspace_creation_url(app_url: &str) -> Result<String> {
    let mut url = Url::parse(app_url).context("parse app URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("app URL must use http or https")
    }
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("app URL cannot be used for workspace creation"))?
        .push("workspaces")
        .push("new");
    Ok(url.to_string())
}

pub(crate) fn validate_login_token(token: &str) -> Result<()> {
    if token.starts_with("mul_") || token.starts_with("mcn_") {
        return Ok(());
    }
    bail!("invalid token format: must start with mul_ or mcn_")
}

pub(crate) async fn run_browser_login(
    server_url: &str,
    app_url: &str,
    callback_host: Option<&str>,
    environment: &Environment,
) -> Result<String> {
    let app_url = (!app_url.trim().is_empty())
        .then_some(app_url)
        .context("No app URL configured. Run 'cordy setup' first.")?;
    let callback_host = callback_host
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or("localhost");
    let bind_addr = if callback_host_is_loopback(callback_host) {
        "127.0.0.1:0"
    } else {
        "0.0.0.0:0"
    };
    let listener = TcpListener::bind(bind_addr)
        .await
        .context("start local login callback server")?;
    let port = listener.local_addr()?.port();
    let mut state_bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut state_bytes);
    let state = state_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let callback_url = format!("http://{callback_host}:{port}/callback");
    let login_url = build_login_url(app_url, &callback_url, &state)?;

    eprintln!("Opening browser to authenticate...");
    if !open_login_url(&login_url) {
        eprintln!("Could not open browser automatically.");
    }
    eprintln!("If the browser did not open, visit:\n  {login_url}");
    if environment.trimmed("SSH_CONNECTION").is_some() && callback_host_is_loopback(callback_host) {
        eprintln!("\nRemote SSH session detected. Forward the callback port before opening the URL:\n  ssh -L {port}:127.0.0.1:{port} <user>@<remote-host>");
    }
    eprintln!("\nWaiting for authentication...");

    let jwt = tokio::time::timeout(
        LOGIN_CALLBACK_TIMEOUT,
        wait_for_login_callback(listener, state),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for authentication"))??;
    let client = ApiClient::new(
        server_url.to_owned(),
        String::new(),
        jwt,
        String::new(),
        String::new(),
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")),
        CLIENT_VERSION,
    )?;
    #[derive(Debug, Deserialize)]
    struct TokenResponse {
        token: String,
    }
    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());
    let pat: TokenResponse = client
        .post_json(
            "/api/tokens",
            &serde_json::json!({"name": format!("CLI ({hostname})"), "expires_in_days": 90}),
        )
        .await
        .map_err(|_| anyhow::anyhow!("server could not issue an access token for the CLI"))?;
    if pat.token.trim().is_empty() {
        bail!("server returned an empty access token")
    }
    validate_login_token(&pat.token)?;
    Ok(pat.token)
}

pub(crate) fn build_login_url(app_url: &str, callback_url: &str, state: &str) -> Result<String> {
    let mut login_url = Url::parse(app_url).context("parse app URL")?;
    if !matches!(login_url.scheme(), "http" | "https") || login_url.host_str().is_none() {
        bail!("app URL must use http or https")
    }
    login_url
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("app URL cannot be used for login"))?
        .push("login");
    login_url
        .query_pairs_mut()
        .append_pair("cli_callback", callback_url)
        .append_pair("cli_state", state);
    Ok(login_url.to_string())
}

pub(crate) fn callback_host_is_loopback(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn open_login_url(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let command = ("open", url);
    #[cfg(target_os = "linux")]
    let command = ("xdg-open", url);
    #[cfg(target_os = "windows")]
    let command = ("rundll32", &format!("url.dll,FileProtocolHandler {url}"));
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        std::process::Command::new(command.0)
            .arg(command.1)
            .spawn()
            .is_ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        false
    }
}

pub(crate) async fn wait_for_login_callback(
    listener: TcpListener,
    expected_state: String,
) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await.context("accept login callback")?;
        let mut request = vec![0_u8; 16 * 1024];
        let read = tokio::time::timeout(LOGIN_CALLBACK_READ_TIMEOUT, stream.read(&mut request))
            .await
            .map_err(|_| anyhow::anyhow!("login callback request timed out"))??;
        let request =
            std::str::from_utf8(&request[..read]).context("invalid login callback request")?;
        let target = request
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|line| line.split_whitespace().next())
            .context("invalid login callback request line")?;
        let callback = Url::parse(&format!("http://localhost{target}"))
            .context("invalid login callback URL")?;
        if callback.path() != "/callback" {
            write_login_response(&mut stream, "404 Not Found", "Not found").await?;
            continue;
        }
        let state = callback
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        let token = callback
            .query_pairs()
            .find(|(key, _)| key == "token")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        if !constant_time_equal(state.as_bytes(), expected_state.as_bytes()) {
            write_login_response(&mut stream, "400 Bad Request", "Invalid callback state").await?;
            continue;
        }
        if token.trim().is_empty() {
            write_login_response(&mut stream, "400 Bad Request", "Missing token").await?;
            continue;
        }
        write_login_response(
            &mut stream,
            "200 OK",
            "Authentication successful. You can close this tab and return to the terminal.",
        )
        .await?;
        return Ok(token);
    }
}

pub(crate) fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

async fn write_login_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("write login callback response")?;
    Ok(())
}
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct AuthUser {
    pub(super) name: String,
    pub(super) email: String,
}

/// Authenticate either with a PAT or with the browser callback flow. The
/// credential and workspace reset are committed together after the new token
/// has been verified, so a failed login cannot damage an existing profile.
pub(super) async fn run_login(cli: &Cli, environment: &Environment, args: &LoginArgs) -> Result<RunOutput> {
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
